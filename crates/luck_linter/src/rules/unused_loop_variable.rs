use luck_diagnostics::Tag;
use luck_semantic::SymbolKind;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};
use crate::underscore_fix::prefix_underscore;

/// Flags a `for` loop variable that's never read (Luacheck 213).
///
/// It's separate from `unused_variable`, which skips loop variables, so a
/// project can allow loops that only count iterations and still catch unused
/// locals.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedLoopVariable;

declare_lint_rule!(
    UnusedLoopVariable,
    Style,
    Warning,
    "for-loop variable is declared but never read in the loop body",
    fix = Suggestion
);

impl Rule for UnusedLoopVariable {
    fn run_once(&self, context: &LintContext<'_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();

        for symbol in tree.symbols() {
            if !matches!(symbol.kind, SymbolKind::IteratorVariable | SymbolKind::NumericForVariable)
            {
                continue;
            }
            let name = tree.name(symbol.name);
            if name.starts_with('_') {
                continue;
            }

            let has_read = tree
                .resolved_references(symbol.id)
                .iter()
                .any(|&reference_id| tree.reference(reference_id).kind.is_read());
            if has_read {
                continue;
            }

            context.diagnostic(
                Finding::new(format!("unused loop variable `{name}`"), symbol.definition_span)
                    .with_tag(Tag::Unnecessary)
                    .with_help("prefix with `_` to suppress this warning")
                    .with_fix(context, FixKind::Suggestion, |fixer| {
                        prefix_underscore(fixer, semantic, symbol)
                    }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::{applied, fix_and_reparse, run_rule};
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::UnusedLoopVariable, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_and_fixes_with_underscore_prefix() {
        let source = "for i = 1, 10 do print() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "for _i = 1, 10 do print() end");
    }

    #[test]
    fn flags_numeric_for_unused() {
        let diags = run("for i = 1, 10 do print() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`i`"));
    }

    #[test]
    fn ignores_underscore_numeric_for() {
        let diags = run("for _ = 1, 10 do print() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_generic_for_key_only() {
        let diags = run("for k, v in pairs(t) do print(v) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`k`"));
    }

    #[test]
    fn ignores_used_numeric_for() {
        let diags = run("for i = 1, 10 do print(i) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_both_unused_generic_for() {
        let diags = run("for k, v in pairs(t) do print() end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn flags_without_fix_that_captures_existing_underscore_name() {
        let source = "\
local _k = 1
for k, v in pairs({}) do print(_k, v) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            applied(source, &diags),
            source,
            "the rename shouldn't make `print(_k)` read the loop variable"
        );
    }

    #[test]
    fn flags_without_fix_when_loop_variable_is_written() {
        let diags = run("for i = 1, 3 do i = 5 end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].fix.is_none(),
            "renaming the loop variable makes `i = 5` global: {diags:?}"
        );
    }
}
