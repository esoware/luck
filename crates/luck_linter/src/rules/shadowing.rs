use luck_semantic::SymbolKind;

use crate::diagnostic::{Finding, line_column};
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Shadowing;

impl_rule!(
    Shadowing,
    "shadowing",
    Style,
    Warning,
    "variable shadows an outer variable with the same name"
);

impl WholeRule for Shadowing {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let mut findings = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            let Some(shadowed_id) = symbol.shadows else {
                continue;
            };
            // A leading `_` marks a name as deliberately unused, and an
            // implicit `self` has no name the author could change.
            if symbol.name.starts_with('_') || symbol.kind == SymbolKind::ImplicitSelf {
                continue;
            }
            let shadowed = semantic.scope_tree.symbol(shadowed_id);
            let (line, column) =
                line_column(context.source, shadowed.definition_span.start, semantic.version);
            findings.push(
                Finding::new(
                    format!("variable `{}` shadows outer variable", symbol.name),
                    symbol.definition_span,
                )
                .with_help(format!("outer variable defined at {line}:{column}")),
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
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&Shadowing, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_nested_block_shadow() {
        // Inner-scope redeclaration of an outer local. `redefining_local`
        // covers only the same block.
        let diags = run("local x = 1\ndo local x = 2 print(x) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn flags_loop_variable_shadow() {
        let diags = run("local i = 1\nfor i = 1, 3 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_distinct_names() {
        let diags = run("local x = 1\ndo local y = 2 print(y) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_underscore_names() {
        let diags = run("local _ = 1\ndo local _ = 2 end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_implicit_self_of_nested_method() {
        let diags = run(
            "local M = {}\nfunction M:a()\n local t = {}\n function t:b() return self end\n return t\nend\nreturn M",
        );
        assert!(diags.is_empty(), "an implicit `self` has no name to change: {diags:?}");
    }

    #[test]
    fn ignores_local_arg_in_a_lua51_vararg_function() {
        let source = "local function f(...) local arg = { ... } return arg end";
        let diags = run_rule(&Shadowing, source, LuaVersion::Lua51);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
