use luck_ast::{Field, TableConstructor};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags table constructors that mix positional fields with `name = value`
/// fields, which `ipairs` skips.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MixedTable;

impl_rule!(
    MixedTable,
    "mixed_table",
    Suspicious,
    Warning,
    "table constructor mixes positional and record-style fields"
);

impl NodeRule for MixedTable {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::TableConstructor])
    }
    fn on_table(
        &self,
        table: &TableConstructor,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
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
            findings.push(
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
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
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

    #[test]
    fn flags_mixed_table_in_table_call_argument() {
        let diags = run("foo{ 1, x = 2 }");
        assert_eq!(diags.len(), 1, "a `f{{...}}` table is a table constructor: {diags:?}");
    }
}
