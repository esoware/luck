use luck_ast::{Expression, Field, NodeType, NodeTypeSet};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};

/// Flags table constructors that mix positional fields with `name = value`
/// fields, which `ipairs` skips.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MixedTable;

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
}

impl NodeRule for MixedTable {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::TableConstructor])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        let Expression::TableConstructor(table) = expr else {
            return;
        };
        let mut has_positional = false;
        let mut has_named = false;
        for field in table.fields.iter() {
            match field {
                Field::Positional { .. } => has_positional = true,
                Field::Named { .. } => has_named = true,
                // An explicit `[expr] = v` key shows the author chose where
                // the value goes, so it counts as neither kind.
                Field::Bracketed { .. } => {}
            }
        }
        if has_positional && has_named {
            diags.push(
                LintDiagnostic::new(
                    "mixed_table",
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
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&MixedTable, source, LuaVersion::Lua54)
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
        // `[3] = "x"` is explicit numeric placement the author opted into.
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
