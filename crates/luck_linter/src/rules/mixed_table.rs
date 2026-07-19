use luck_ast::Expression;
use luck_ast::shared::Field;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

/// Mixing positional fields and identifier-keyed record fields in one
/// table constructor confuses readers and breaks `ipairs` expectations:
/// the positional entries land in the sequence half, the record entries
/// in the hash half, and only the sequence half iterates predictably.
/// Explicit bracket-indexed keys are exempt because those are the
/// documented way to put arbitrary keys alongside sequence entries.
pub struct MixedTable;

impl Rule for MixedTable {
    fn name(&self) -> &'static str {
        "mixed_table"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "table constructor mixes positional and record-style fields"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for MixedTable {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::TableConstructor]);
        Some(&TYPES)
    }
    fn on_expression(
        &self,
        expr: &luck_ast::expr::Expression,
        _ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let Expression::TableConstructor(table) = expr {
            let mut has_positional = false;
            let mut has_named = false;
            for (field, _) in &table.fields {
                match field {
                    Field::Positional { .. } => has_positional = true,
                    Field::Named { .. } => has_named = true,
                    // Bracketed keys (`[expr] = v`) signal explicit
                    // author intent about where the value lands. Do not
                    // count them on either side.
                    Field::Bracketed { .. } => {}
                }
            }
            if has_positional && has_named {
                out.push(LintDiagnostic::new("mixed_table", "table mixes positional and record fields; iteration with `ipairs` will skip the record fields"
                            .to_string(), table.span).with_help(
                        "split into separate sequence and record tables, or use explicit `[N] = v` keys"
                            .to_string(),
                    ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&MixedTable, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_positional_with_named() {
        let diags = run("local t = {1, 2, foo = 3}");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_pure_positional() {
        let diags = run("local t = {1, 2, 3}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_pure_record() {
        let diags = run("local t = {a = 1, b = 2}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_positional_with_bracketed_key() {
        // `[3] = "x"` is explicit numeric placement - author opted in.
        let diags = run(r#"local t = {1, 2, [3] = "x"}"#);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_empty_table() {
        let diags = run("local t = {}");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_named_then_positional() {
        let diags = run("local t = {foo = 1, 2, 3}");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }
}
