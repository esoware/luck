use luck_semantic::SymbolKind;

use crate::diagnostic::Finding;
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
        let tree = context.semantic.scoping();
        let mut findings = Vec::new();

        for symbol in tree.symbols() {
            let Some(shadowed_id) = symbol.shadows else {
                continue;
            };
            // A leading `_` marks a name as deliberately unused, and an
            // implicit `self` has no name the author could change. Lua 5.1's
            // `local arg = {...}` is how a vararg function replaces its
            // implicit `arg`.
            let name = tree.name(symbol.name);
            if name.starts_with('_')
                || symbol.kind.is_implicit()
                || tree.symbol(shadowed_id).kind == SymbolKind::ImplicitArg
            {
                continue;
            }
            findings.push(
                Finding::new(
                    format!("variable `{name}` shadows outer variable"),
                    symbol.definition_span,
                )
                .with_label(
                    tree.symbol(shadowed_id).definition_span,
                    "outer variable defined here",
                ),
            );
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
        run_rule(&Shadowing, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_nested_block_shadow() {
        // Inner-scope redeclaration of an outer local. `redefining_local`
        // covers only the same block.
        let source = "\
local x = 1
do local x = 2 print(x) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`x`"));
        assert_eq!(diags[0].labels.first().map(|label| label.span.start), Some(6));
    }

    #[test]
    fn flags_loop_variable_shadow() {
        let diags = run("\
local i = 1
for i = 1, 3 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_distinct_names() {
        let diags = run("\
local x = 1
do local y = 2 print(y) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_underscore_names() {
        let diags = run("\
local _ = 1
do local _ = 2 end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_implicit_self_of_nested_method() {
        let diags = run("\
local M = {}
function M:a()
 local t = {}
 function t:b() return self end
 return t
end
return M");
        assert!(diags.is_empty(), "an implicit `self` has no name to change: {diags:?}");
    }

    #[test]
    fn ignores_local_arg_in_a_lua51_vararg_function() {
        let source = "local function f(...) local arg = { ... } return arg end";
        let diags = run_rule(&Shadowing, source, LuaVersion::Lua51);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
