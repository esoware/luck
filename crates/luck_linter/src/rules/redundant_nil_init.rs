use luck_ast::Expression;
use luck_ast::stmt::LocalAssignment;
use luck_ast::visitor::Visitor;
use luck_token::Span;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// `local x = nil` is equivalent to `local x` in Lua semantics. The
/// `= nil` adds no information and only costs source-code bytes.
pub struct RedundantNilInit;

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
        "local variable initialized to nil (the default value)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = NilInitChecker {
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct NilInitChecker {
    diagnostics: Vec<LintDiagnostic>,
}

impl NilInitChecker {
    fn check_local(&mut self, local: &LocalAssignment) {
        let Some((equal, exprs)) = &local.equal_and_exprs else {
            return;
        };

        let values: Vec<&Expression> = exprs.iter().collect();

        if values.is_empty() {
            return;
        }

        let mut trailing_nils = 0usize;
        for expr in values.iter().rev() {
            if matches!(expr, Expression::Nil(_)) {
                trailing_nils += 1;
            } else {
                break;
            }
        }
        if trailing_nils == 0 {
            return;
        }

        let names_count = local.names.iter().count();
        let all_nil = trailing_nils == values.len();

        if all_nil {
            let edit_start = last_name_end_byte(local);
            let edit_end = local.span.end;
            self.diagnostics.push(
                LintDiagnostic::new(
                    "redundant_nil_init",
                    "redundant `= nil` initializer; uninitialized locals are nil".to_string(),
                    local.span,
                )
                .with_help("drop the `= nil` to rely on the default value".to_string())
                .with_fix(Fix {
                    description: "drop redundant `= nil` initializer".to_string(),
                    edits: vec![TextEdit {
                        span: Span::new(edit_start, edit_end),
                        replacement: String::new(),
                    }],
                }),
            );
            let _ = equal;
            return;
        }

        // Mixed case. Lua's multi-return rule: an expression in the
        // last position of a value list expands to all its return
        // values; in any earlier position it's truncated to one. So
        // dropping a trailing `nil` is only safe if the new last
        // expression isn't a multi-return-capable form that wasn't
        // already truncated. We require the expression that would
        // become the new last value to be neither a function call nor
        // a vararg.
        let target_keep = values.len() - trailing_nils;
        if target_keep == 0 {
            // Handled by the all-nil branch above.
            return;
        }
        let new_last = values[target_keep - 1];
        if matches!(
            new_last,
            Expression::FunctionCall(_) | Expression::VarArg(_)
        ) {
            return;
        }
        let _ = names_count;

        // Range to delete: from the comma BEFORE the first dropped value
        // through the end of the last value.
        let comma_byte = exprs.items[target_keep - 1]
            .1
            .as_ref()
            .expect("kept expression is followed by a comma")
            .span
            .start;
        let total_end = exprs
            .last_item()
            .map(|e| e.span().end)
            .unwrap_or(comma_byte);

        self.diagnostics.push(
            LintDiagnostic::new(
                "redundant_nil_init",
                format!(
                    "{} trailing `nil` value{} in local initializer",
                    trailing_nils,
                    if trailing_nils == 1 { "" } else { "s" }
                ),
                local.span,
            )
            .with_help("drop trailing `nil` values; the locals default to nil".to_string())
            .with_fix(Fix {
                description: "drop trailing `nil` initializers".to_string(),
                edits: vec![TextEdit {
                    span: Span::new(comma_byte, total_end),
                    replacement: String::new(),
                }],
            }),
        );
    }
}

/// Byte offset just past the last name (or attribute) in the binding list.
fn last_name_end_byte(local: &LocalAssignment) -> u32 {
    let mut end: u32 = local
        .names
        .last_item()
        .map(|attributed| attributed.name.span.end)
        .unwrap_or(local.span.start);
    for attrib in local.names.iter().filter_map(|n| n.attrib.as_ref()) {
        if attrib.span.end > end {
            end = attrib.span.end;
        }
    }
    end
}

impl Visitor for NilInitChecker {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        if let luck_ast::Statement::LocalAssignment(local) = stmt {
            self.check_local(local);
        }
        self.walk_statement(stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&RedundantNilInit, source, LuaVersion::Lua54)
    }

    fn apply(source: &str, diag: &LintDiagnostic) -> String {
        let fix = diag.fix.as_ref().expect("fix");
        let edit = &fix.edits[0];
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..edit.span.start as usize]);
        out.push_str(&edit.replacement);
        out.push_str(&source[edit.span.end as usize..]);
        let parse = luck_parser::parse(&out, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
        out
    }

    #[test]
    fn flags_single_nil() {
        let source = "local x = nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "expected one diag, got {diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "local x");
    }

    #[test]
    fn flags_all_nil_multi() {
        let source = "local a, b = nil, nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "local a, b");
    }

    #[test]
    fn flags_trailing_nil_only() {
        let source = "local a, b = 1, nil";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
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
    fn keeps_trailing_nil_after_function_call() {
        // Dropping the nil here would change `b` from `nil` to the
        // second return value of `f()`.
        let diags = run("local a, b = f(), nil");
        assert!(
            diags.is_empty(),
            "must not drop nil after call; got {diags:?}"
        );
    }

    #[test]
    fn keeps_trailing_nil_after_vararg() {
        let diags = run("local function g(...) local a, b = ..., nil end");
        assert!(
            diags.is_empty(),
            "must not drop nil after vararg; got {diags:?}"
        );
    }
}
