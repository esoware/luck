use luck_semantic::SymbolKind;

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags a function parameter that's never referenced (Luacheck 212).
///
/// Callbacks and methods often ignore parameters their interface requires,
/// so this has its own toggle, separate from `unused_variable`, which skips
/// parameters.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnusedArgument;

impl_rule!(
    UnusedArgument,
    "unused_argument",
    Style,
    Warning,
    "function parameter is declared but never referenced"
);

impl WholeRule for UnusedArgument {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let mut findings = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.kind != SymbolKind::Parameter {
                continue;
            }
            // A parameter spelled `self` is a receiver the interface requires.
            if symbol.name == "self" {
                continue;
            }
            if symbol.name.starts_with('_') {
                continue;
            }
            if !symbol.reference_ids.is_empty() {
                continue;
            }

            findings.push(
                Finding::new(format!("unused argument `{}`", symbol.name), symbol.definition_span)
                    .with_help("prefix with `_` to suppress this warning")
                    .with_fix(Fix {
                        description: format!("prefix `{}` with `_`", symbol.name),
                        edits: vec![TextEdit {
                            span: symbol.definition_span,
                            replacement: format!("_{}", symbol.name),
                        }],
                    }),
            );
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fix::apply_one_pass;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnusedArgument, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_unused_named_vararg() {
        // Lua 5.5 named vararg binds like a parameter.
        let source = "local function f(...args) end";
        let diags = run_rule(&UnusedArgument, source, LuaVersion::Lua55);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua55);
        assert_eq!(fixed, "local function f(..._args) end");
    }

    #[test]
    fn ignores_used_named_vararg() {
        let diags = run_rule(
            &UnusedArgument,
            "local function f(...args) return args end",
            LuaVersion::Lua55,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_with_underscore_prefix() {
        let source = "local function f(x) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local function f(_x) end");
    }

    #[test]
    fn flags_unused_parameter() {
        let diags = run("local function f(x) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local function f(_x) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_used_parameter() {
        let diags = run("local function f(x) return x end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_method_arg_not_self() {
        let source = "local obj = {}\nfunction obj:method(x) end";
        let diags = run(source);
        // Only `x` is flagged. `self` is the implicit method receiver.
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
        assert!(!diags[0].message.contains("self"));
    }

    #[test]
    fn flags_multiple_parameters_some_used() {
        let diags = run("local function f(a, b, c) return a + c end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`b`"));
    }

    #[test]
    fn bug_fix_does_not_duplicate_existing_underscore_parameter() {
        let source = "local function f(_a, a) return _a end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_ne!(
            apply_one_pass(source, &diags),
            "local function f(_a, _a) return _a end",
            "the rename shouldn't make `return _a` read the second parameter"
        );
    }
}
