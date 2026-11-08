use luck_ast::{Expression, LocalAssignment, Statement, is_multi_value};
use luck_token::Span;

use crate::diagnostic::{Category, Finding, Severity};
use crate::fix::{Fix, TextEdit};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

/// Flags `nil` initializers on locals, as in `local x = nil` or the trailing
/// `nil` in `local a, b = 1, nil`, since a local starts out `nil` anyway.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RedundantNilInit;

impl Rule for RedundantNilInit {
    fn name(&self) -> &'static str {
        "redundant_nil_init"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "local variable initialized to `nil` (the default value)"
    }
}

impl NodeRule for RedundantNilInit {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::LocalAssignment])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Statement::LocalAssignment(local) = stmt {
            findings.extend(check_local(local));
        }
    }
}

fn check_local(local: &LocalAssignment) -> Option<Finding> {
    let exprs = local.exprs.as_ref()?;
    let trailing_nil_count =
        exprs.items.iter().rev().take_while(|expr| matches!(expr, Expression::Nil(_))).count();
    if trailing_nil_count == 0 {
        return None;
    }

    let Some(last_kept_expr) = exprs.items.iter().rev().nth(trailing_nil_count) else {
        return Some(
            Finding::new(
                "redundant `= nil` initializer, because uninitialized locals are `nil`",
                local.span,
            )
            .with_help("drop the `= nil` to rely on the default value")
            .with_fix(Fix {
                description: "drop redundant `= nil` initializer".to_owned(),
                edits: vec![TextEdit {
                    span: Span::new(last_name_end(local), local.span.end),
                    replacement: String::new(),
                }],
            }),
        );
    };

    // A call or `...` that ended up last would expand to fill the names the
    // `nil`s covered, so leave those alone.
    if is_multi_value(last_kept_expr) {
        return None;
    }

    // Delete from the end of the last kept value, comma included, through
    // the end of the list.
    let delete_start = last_kept_expr.span().end;
    let delete_end = exprs.last().map_or(delete_start, |last_expr| last_expr.span().end);
    Some(
        Finding::new(
            format!(
                "{trailing_nil_count} trailing `nil` value{} in local initializer",
                if trailing_nil_count == 1 { "" } else { "s" }
            ),
            local.span,
        )
        .with_help("drop trailing `nil` values, because the locals default to `nil`")
        .with_fix(Fix {
            description: "drop trailing `nil` initializers".to_owned(),
            edits: vec![TextEdit {
                span: Span::new(delete_start, delete_end),
                replacement: String::new(),
            }],
        }),
    )
}

/// Byte offset just past the last name (or attribute) in the binding list.
fn last_name_end(local: &LocalAssignment) -> u32 {
    let last_name_end =
        local.names.last().map_or(local.span.start, |attributed| attributed.name.span.end);
    local
        .names
        .iter()
        .filter_map(|attributed| attributed.attribute.as_ref())
        .map(|attribute| attribute.span.end)
        .fold(last_name_end, u32::max)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&RedundantNilInit, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_single_nil() {
        let source = "local x = nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "expected one diag, got {diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x");
    }

    #[test]
    fn flags_multiple_nil_values() {
        let source = "local a, b = nil, nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b");
    }

    #[test]
    fn flags_and_fixes_keeping_attribute() {
        let source = "local x <const> = nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local x <const>");
    }

    #[test]
    fn flags_trailing_nil_only() {
        let source = "local a, b = 1, nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local a, b = 1");
    }

    #[test]
    fn ignores_non_nil_rhs() {
        let diags = run("local x = 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_uninitialized() {
        let diags = run("local x");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_leading_nil_when_more_values_follow() {
        let diags = run("local a, b = nil, 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_trailing_nil_after_function_call() {
        // Dropping the nil here would change `b` from `nil` to the
        // second return value of `f()`.
        let diags = run("local a, b = f(), nil");
        assert!(diags.is_empty(), "must not drop nil after call; got {diags:?}");
    }

    #[test]
    fn ignores_trailing_nil_after_vararg() {
        let diags = run("local function g(...) local a, b = ..., nil end");
        assert!(diags.is_empty(), "must not drop nil after vararg; got {diags:?}");
    }
}
