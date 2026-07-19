use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct Shadowing;

impl Rule for Shadowing {
    fn name(&self) -> &'static str {
        "shadowing"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "variable shadows an outer variable with the same name"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let _block = ctx.block;
        let semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if let Some(shadowed_id) = symbol.shadows {
                // Underscore-prefixed names are a deliberate discard convention, not real shadowing.
                if symbol.name == "_" || symbol.name.starts_with('_') {
                    continue;
                }
                let shadowed = &semantic.scope_tree.symbols[shadowed_id.index()];
                diagnostics.push(
                    LintDiagnostic::new(
                        "shadowing",
                        format!("variable '{}' shadows outer variable", symbol.name),
                        symbol.definition_span,
                    )
                    .with_help(format!(
                        "outer variable defined at offset {}",
                        shadowed.definition_span.start
                    )),
                );
            }
        }

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&Shadowing, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_nested_block_shadow() {
        // Inner-scope redeclaration of an outer local. Distinct from
        // redefining_local, which is same-block only.
        let diags = run("local x = 1\ndo local x = 2 print(x) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("'x'"));
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
}
