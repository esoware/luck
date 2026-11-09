use luck_ast::{Expression, FunctionAttribute, LastStatement, Statement};
use luck_token::{CommentKind, Span, offset_to_index};

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RedundantNativeAttribute;

impl_rule!(
    RedundantNativeAttribute,
    "redundant_native_attribute",
    Style,
    Warning,
    "`@native` attribute is redundant in a `--!native` module"
);

impl NodeRule for RedundantNativeAttribute {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[
            NodeType::FunctionDecl,
            NodeType::LocalFunction,
            NodeType::FunctionDef,
        ])
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let attributes = match stmt {
            Statement::FunctionDecl(function_decl) => &function_decl.attributes,
            Statement::LocalFunction(local_function) => &local_function.attributes,
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => return,
        };
        check_attributes(attributes, context, findings);
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Expression::FunctionDef(function) = expr {
            check_attributes(&function.attributes, context, findings);
        }
    }
}

fn check_attributes(
    attributes: &[FunctionAttribute],
    context: &LintContext<'_>,
    findings: &mut Vec<Finding>,
) {
    // Check for attributes first, since the directive scan reads the
    // comments.
    if attributes.is_empty()
        || !context.semantic.version.is_luau()
        || !has_native_directive(context)
    {
        return;
    }
    for attribute in attributes {
        if attribute.name.identifier() != Some("native") {
            continue;
        }
        findings.push(
            Finding::new(
                "`@native` attribute is redundant in a `--!native` module",
                attribute.span,
            )
            .with_optional_fix(removal_fix(attribute, attributes, context.source)),
        );
    }
}

/// Deletes `attribute` and one whitespace byte after it, so no doubled
/// separator is left behind. A `@[...]` list keeps the fix only when
/// `attribute` is its sole entry and nothing but its name sits in brackets.
fn removal_fix(
    attribute: &FunctionAttribute,
    attributes: &[FunctionAttribute],
    source: &str,
) -> Option<Fix> {
    let bytes = source.as_bytes();
    let after_at = offset_to_index(attribute.span.start).saturating_add(1);
    let mut edit_end = offset_to_index(attribute.name.span.end);
    if bytes.get(after_at) == Some(&b'[') {
        if attribute.args.is_some()
            || attributes.iter().filter(|other| other.span.start == attribute.span.start).count()
                > 1
        {
            return None;
        }
        let whitespace_length = bytes
            .get(edit_end..)
            .unwrap_or_default()
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
        let close_index = edit_end.saturating_add(whitespace_length);
        if bytes.get(close_index) != Some(&b']') {
            return None;
        }
        edit_end = close_index.saturating_add(1);
    }
    let has_trailing_space = matches!(bytes.get(edit_end), Some(b' ' | b'\t' | b'\n' | b'\r'));
    let span = Span::from_indices(
        offset_to_index(attribute.span.start),
        edit_end.saturating_add(usize::from(has_trailing_space)),
    );
    Some(Fix {
        description: "remove the redundant `@native` attribute".to_owned(),
        edits: vec![TextEdit { span, replacement: String::new() }],
    })
}

/// Whether a `--!native` hot comment comes before any code. Luau ignores one
/// placed later.
fn has_native_directive(context: &LintContext<'_>) -> bool {
    let block = context.block;
    let first_code_start = block
        .stmts
        .first()
        .map(Statement::span)
        .or_else(|| block.last_stmt.as_deref().map(LastStatement::span))
        .map(|span| span.start);
    // Comments come in source order, so the scan stops at the first code.
    context
        .comments
        .iter()
        .take_while(|comment| first_code_start.is_none_or(|start| comment.span.start <= start))
        .any(|comment| {
            comment.kind == CommentKind::Line
                && comment
                    .span
                    .text(context.source)
                    .strip_prefix("--!")
                    .is_some_and(|body| body.trim_end() == "native")
        })
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&RedundantNativeAttribute, source, LuaVersion::Luau)
    }

    #[test]
    fn flags_function_decl_attribute() {
        let source = "--!native\n@native function f() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "--!native\nfunction f() end");
    }

    #[test]
    fn flags_local_function_attribute() {
        let source = "--!native\n@native\nlocal function f() end\nf()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "--!native\nlocal function f() end\nf()");
    }

    #[test]
    fn flags_nested_function_attribute() {
        let source = "--!native\ndo\n@native function f() end\nend";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        fix_and_reparse(source, &diags, LuaVersion::Luau);
    }

    #[test]
    fn flags_function_expr_attribute() {
        let source = "--!native\n@native\nlocal function a() end\nlocal f = @native function() end";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "--!native\nlocal function a() end\nlocal f = function() end");
    }

    #[test]
    fn ignores_without_native_directive() {
        let diags = run("@native function f() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_misplaced_native_directive() {
        let diags = run("local _x = 1\n--!native\n@native function f() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_other_attributes() {
        let diags = run("--!native\n@checked function f() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_luau() {
        let diags =
            run_rule(&RedundantNativeAttribute, "--!native\nlocal _x = 1", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_bracketed_attribute() {
        let source = "--!native\n@[native] function f() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(
            fixed, "--!native\nfunction f() end",
            "the fix should remove the whole attribute"
        );
    }

    #[test]
    fn ignores_native_directive_with_trailing_text() {
        let diags = run("--!native on\n@native function f() end");
        assert!(diags.is_empty(), "the compiler ignores `--!native on`: {diags:?}");
    }

    #[test]
    fn flags_without_fix_when_attribute_shares_a_list() {
        let diags = run("--!native\n@[native, deprecated] function f() end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn flags_with_directive_followed_by_trailing_space() {
        let diags = run("--!native \n@native function f() end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }
}
