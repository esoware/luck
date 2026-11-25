use luck_allocator::Allocator;
use luck_ast::{Chunk, GetSpan as _, TreeCounts};
use luck_diagnostics::Diagnostic;
use luck_semantic::SemanticBuilder;
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::dynamic_require::DynamicRequire;
use crate::exports::Exports;

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
    /// In source order.
    pub diags: Vec<Diagnostic>,
    pub exports: Exports<'ast>,
}

/// What bundling needs from `chunk`, whose text is `source`, read off the one
/// semantic pass that finds the calls of the global `require`, with tables
/// sized from the parse's `counts`.
///
/// A read of a name `require` that bundling leaves alone gets W005: one of
/// the global `require` that doesn't call it, since a call through what it
/// reads escapes bundling, and one of any other `require`, since a call of it
/// keeps its meaning.
///
/// `dynamic_require` decides whether a `require(expr)` aborts the bundle or
/// stays as written for the runtime. Each require string goes in
/// `allocator`, since it outlives the borrow of the tree that the analysis
/// reads, which the emitters move and rewrite.
pub(crate) fn analyze<'ast>(
    allocator: &'ast Allocator,
    chunk: &Chunk<'_>,
    counts: TreeCounts,
    source: &'ast [u8],
    dynamic_require: DynamicRequire,
) -> Analysis<'ast> {
    let semantic =
        SemanticBuilder::new().with_counts(counts).with_requires(true).build(chunk).semantic;
    let scoping = semantic.scoping();
    let mut requires = Vec::new();
    let mut diags = Vec::new();
    // The call that first required each string.
    let mut first_requires: FxHashMap<&str, Span> = FxHashMap::default();
    for site in semantic.requires() {
        let call_span = site.call.span();
        match site.path.and_then(|literal| Some((literal.as_str()?, literal.span))) {
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
            // default it stays and resolves at runtime. Luau keys its cache by
            // resolved file, and a Roblox require takes an Instance, so the
            // bundle has nothing to look a runtime argument up in.
            None => match dynamic_require {
                DynamicRequire::Error => diags.push(Diagnostic::e002(call_span)),
                DynamicRequire::Warn => diags.push(Diagnostic::w007(call_span)),
                DynamicRequire::Allow => {}
            },
        }
    }

    if let Some(require) = scoping.name_id("require") {
        // Where each call of the global `require` by its name starts, which is
        // where the name it calls does.
        let called: Vec<u32> = semantic
            .requires()
            .iter()
            .filter_map(|site| site.call.callee.as_name())
            .map(|callee| callee.span.start)
            .collect();
        for reference in scoping.references() {
            if reference.name == require
                && reference.kind.is_read()
                && called.binary_search(&reference.span.start).is_err()
            {
                diags.push(if scoping.is_host_global(reference) {
                    Diagnostic::w005_aliased(reference.span)
                } else {
                    Diagnostic::w005_shadowed(reference.span)
                });
            }
        }
    }
    diags.sort_by_key(|diag| diag.span.map(|span| span.start));

    Analysis { requires, diags, exports: Exports::new(&chunk.block, scoping, source) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_with<'ast>(
        allocator: &'ast Allocator,
        source: &'ast str,
        dynamic_require: DynamicRequire,
    ) -> Analysis<'ast> {
        let parsed = luck_parser::parse(allocator, source.as_bytes());
        assert!(parsed.errors.is_empty(), "parse failed: {:?}", parsed.errors);
        analyze(allocator, &parsed.chunk, parsed.counts, source.as_bytes(), dynamic_require)
    }

    fn analyze_source<'ast>(allocator: &'ast Allocator, source: &'ast str) -> Analysis<'ast> {
        analyze_with(allocator, source, DynamicRequire::default())
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
        let analysis = analyze_source(
            &allocator,
            "\
local utils = require(\"./utils\")
print(utils.foo())
",
        );
        assert_eq!(require_strings(&analysis), ["./utils"]);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn extracts_multiple_requires_in_order() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
local a = require(\"./mod_a\")
local b = require(\"./mod_b\")
local c = require(\"./mod_c\")
print(a, b, c)
",
        );
        assert_eq!(require_strings(&analysis), ["./mod_a", "./mod_b", "./mod_c"]);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn require_after_code_is_not_flagged() {
        let allocator = Allocator::new();
        // Requires are position-independent with the lazy loader.
        let analysis = analyze_source(
            &allocator,
            "\
print(\"hello\")
local x = require(\"./x\")
",
        );
        assert_eq!(analysis.requires.len(), 1);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn non_literal_require_flags_e002_in_error_mode() {
        let allocator = Allocator::new();
        let source = "local x = require(varname)\n";
        let analysis = analyze_with(&allocator, source, DynamicRequire::Error);
        assert!(analysis.requires.is_empty());
        assert_eq!(with_code(&analysis, "E002").len(), 1, "{:?}", analysis.diags);
    }

    /// The default: one runtime-resolved require must not abort the bundle.
    #[test]
    fn non_literal_require_warns_and_is_kept_by_default() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "local x = require(varname)\n");
        assert!(analysis.requires.is_empty());
        assert!(with_code(&analysis, "E002").is_empty(), "{:?}", analysis.diags);
        assert_eq!(with_code(&analysis, "W007").len(), 1, "{:?}", analysis.diags);
    }

    #[test]
    fn allow_mode_keeps_the_call_without_warning() {
        let allocator = Allocator::new();
        let source = "local x = require(varname)\n";
        let analysis = analyze_with(&allocator, source, DynamicRequire::Allow);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    /// A static require nested inside a dynamic one still bundles.
    #[test]
    fn static_require_nested_in_a_dynamic_one_is_still_extracted() {
        let allocator = Allocator::new();
        let analysis =
            analyze_source(&allocator, "local x = require(require(\"./names\").first)\n");
        assert_eq!(require_strings(&analysis), ["./names"]);
        assert_eq!(with_code(&analysis, "W007").len(), 1, "{:?}", analysis.diags);
    }

    #[test]
    fn bare_require_stmt_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "require(\"./something\")\n");
        assert_eq!(require_strings(&analysis), ["./something"]);
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn escape_sequences_decode_like_real_luau() {
        let allocator = Allocator::new();
        // require("./a\46b") is require("./a.b") at runtime; the bundler must
        // resolve and deduplicate on the decoded value.
        let analysis = analyze_source(
            &allocator,
            "\
local a = require(\"./a\\46b\")
local b = require(\"./a.b\")
",
        );
        assert_eq!(require_strings(&analysis), ["./a.b", "./a.b"]);
        // Same decoded module twice: the duplicate-require warning fires.
        assert_eq!(with_code(&analysis, "W001").len(), 1, "{:?}", analysis.diags);
    }

    #[test]
    fn long_string_leading_newline_is_stripped() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "local m = require [[\n./mymod]]\n");
        assert_eq!(require_strings(&analysis), ["./mymod"]);
    }

    #[test]
    fn shadowed_require_is_skipped_with_w005() {
        let allocator = Allocator::new();
        let source = "\
local require = function(s) return s end
local x = require(\"./dep\")
";
        let analysis = analyze_source(&allocator, source);
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
local a = require(\"./real\")
local function f()
    local require = print
    require(\"./fake\")
end
";
        let analysis = analyze_source(&allocator, source);
        assert_eq!(require_strings(&analysis), ["./real"]);
        assert_eq!(with_code(&analysis, "W005").len(), 1, "{:?}", analysis.diags);
    }

    /// The common `local require = require` hides the global from every
    /// call after it, which W005 says at the alias and at each call.
    #[test]
    fn a_local_alias_of_require_flags_w005_where_it_reads_and_is_read() {
        let allocator = Allocator::new();
        let source = "\
local require = require
local a, b = require(\"./a\"), require
";
        let analysis = analyze_source(&allocator, source);
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
        let analysis = analyze_source(
            &allocator,
            "\
local r = require
local x = r(\"./dep\")
",
        );
        assert!(analysis.requires.is_empty());
        assert_eq!(with_code(&analysis, "W005").len(), 1, "{:?}", analysis.diags);
    }

    #[test]
    fn direct_calls_do_not_flag_w005() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
local a = require(\"./x\")
require(\"./y\")
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
local a = _G.require(\"./a\")
local b = _G[\"require\"](name)
local c = _G.require
";
        let analysis = analyze_source(&allocator, source);
        assert_eq!(require_strings(&analysis), ["./a"]);
        assert_eq!(flagged(&analysis, source), ["_G[\"require\"](name)"]);
    }

    /// Luau's `require` never reads `package.loaded`, so a write to it means
    /// in a bundle what it means without one.
    #[test]
    fn package_loaded_writes_are_no_concern_of_the_bundle() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
package.loaded[\"mymod\"] = {}
package.loaded.other = {}
",
        );
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn duplicate_require_flags_w001_and_points_at_the_first() {
        let allocator = Allocator::new();
        let source = "\
local a = require(\"./utils\")
local b = require(\"./utils\")
";
        let analysis = analyze_source(&allocator, source);
        assert_eq!(analysis.requires.len(), 2);
        let [warning] = with_code(&analysis, "W001")[..] else {
            panic!("{:?}", analysis.diags);
        };
        let labeled: Vec<&[u8]> =
            warning.labels.iter().map(|label| label.span.text(source.as_bytes())).collect();
        assert_eq!(labeled, [b"require(\"./utils\")"], "{warning:?}");
    }

    #[test]
    fn top_level_vararg_is_not_flagged() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
local modname = ...
return modname
",
        );
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn no_requires_yields_empty_result() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "print(\"hello world\")\n");
        assert!(analysis.requires.is_empty());
        assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    }

    #[test]
    fn require_string_call_syntax_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "local m = require \"./mymod\"\n");
        assert_eq!(require_strings(&analysis), ["./mymod"]);
    }

    #[test]
    fn single_quoted_require_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "local m = require('./mymod')\n");
        assert_eq!(require_strings(&analysis), ["./mymod"]);
    }

    #[test]
    fn long_bracket_require_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "local m = require [[./mymod]]\n");
        assert_eq!(require_strings(&analysis), ["./mymod"]);
    }

    #[test]
    fn require_in_multi_name_local_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_source(&allocator, "local a, b = require(\"./x\"), 1\n");
        assert_eq!(require_strings(&analysis), ["./x"]);
    }

    #[test]
    fn require_in_nested_function_is_extracted() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
local function setup()
    local m = require(\"./inner\")
end
",
        );
        assert_eq!(require_strings(&analysis), ["./inner"]);
    }

    #[test]
    fn field_function_named_require_is_ignored() {
        let allocator = Allocator::new();
        let analysis = analyze_source(
            &allocator,
            "\
local obj = {}
function obj.require(s) end
obj.require(\"./x\")
",
        );
        assert!(analysis.requires.is_empty());
        assert!(with_code(&analysis, "W005").is_empty(), "{:?}", analysis.diags);
    }
}
