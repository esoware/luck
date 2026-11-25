use luck_ast::{AstKind, AstType, Field};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags table constructors that mix positional fields with `name = value`
/// fields, which `ipairs` skips.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MixedTable;

declare_lint_rule!(
    MixedTable,
    Suspicious,
    Warning,
    "table constructor mixes positional and record-style fields"
);

impl Rule for MixedTable {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::TableConstructor]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::TableConstructor(table) = node.kind() else {
            return;
        };
        let mut has_positional = false;
        let mut has_named = false;
        for field in &table.fields {
            match field {
                Field::Positional(_) => has_positional = true,
                Field::Named { .. } => has_named = true,
                // An explicit `[expr] = v` key shows the author chose where
                // the value goes, so it counts as neither kind.
                Field::Bracketed { .. } => {}
            }
        }
        if has_positional && has_named {
            context.diagnostic(
                Finding::new(
                    "table mixes positional and record fields, so iteration with `ipairs` will \
                     skip the record fields",
                    table.span,
                )
                .with_help(
                    "split into separate sequence and record tables, or use explicit `[N] = v` keys",
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "local t = {1, 2, 3}",
        "local t = {a = 1, b = 2}",
        // `[3] = "x"` is explicit numeric placement the author opted into.
        r#"local t = {1, 2, [3] = "x"}"#,
        "local t = {}",
    ];

    const FAIL: &[&str] = &[
        "local t = {1, 2, foo = 3}",
        "local t = {foo = 1, 2, 3}",
        // A `f{}` table is a table constructor.
        "foo{ 1, x = 2 }",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::MixedTable, PASS, FAIL).test_and_snapshot();
    }
}
