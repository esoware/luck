use luck_ast::Statement;
use luck_ast::stmt::FunctionAttribute;
use luck_token::{CommentKind, Span};

use crate::diagnostic::{Category, Fix, LintDiagnostic, Severity, TextEdit};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct RedundantNativeAttribute;

impl Rule for RedundantNativeAttribute {
    fn name(&self) -> &'static str {
        "redundant_native_attribute"
    }

    fn category(&self) -> Category {
        Category::Style
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "@native attribute is redundant in a --!native module"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

/// Whether the chunk carries a `--!native` hot comment before any code.
/// Luau ignores hot comments placed after the first statement, so a late
/// directive does not make attributes redundant.
fn has_native_directive(ctx: &LintContext) -> bool {
    let stmt_start = ctx.block.stmts.first().map(|stmt| stmt.span().start);
    let last_start = ctx.block.last_stmt.as_ref().map(|last| last.span().start);
    let first_code_start = match (stmt_start, last_start) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    ctx.comments.iter().any(|comment| {
        if comment.kind != CommentKind::Line {
            return false;
        }
        if first_code_start.is_some_and(|start| comment.span.start > start) {
            return false;
        }
        let text = &ctx.source[comment.span.start as usize..comment.span.end as usize];
        let Some(body) = text
            .strip_prefix("--")
            .and_then(|rest| rest.strip_prefix('!'))
        else {
            return false;
        };
        let word_len = body.find(char::is_whitespace).unwrap_or(body.len());
        &body[..word_len] == "native"
    })
}

struct AttributeChecker<'src, 'out> {
    source: &'src str,
    out: &'out mut Vec<LintDiagnostic>,
}

impl AttributeChecker<'_, '_> {
    fn check_attributes(&mut self, attributes: &[FunctionAttribute]) {
        for attribute in attributes {
            let name =
                &self.source[attribute.name.span.start as usize..attribute.name.span.end as usize];
            if name != "native" {
                continue;
            }
            // Eat one trailing whitespace byte so deleting the attribute
            // leaves no doubled separator; the remaining text re-parses
            // either way, this just keeps the output tidy.
            let mut edit_end = attribute.name.span.end;
            if matches!(
                self.source.as_bytes().get(edit_end as usize),
                Some(b' ' | b'\t' | b'\n' | b'\r')
            ) {
                edit_end += 1;
            }
            self.out.push(
                LintDiagnostic::new(
                    "redundant_native_attribute",
                    "@native attribute is redundant in a --!native module",
                    attribute.span,
                )
                .with_fix(Fix {
                    description: "remove the redundant @native attribute".into(),
                    edits: vec![TextEdit {
                        span: Span::new(attribute.span.start, edit_end),
                        replacement: String::new(),
                    }],
                }),
            );
        }
    }
}

impl NodeRule for RedundantNativeAttribute {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::FunctionDecl, NodeType::LocalFunction]);
        Some(&TYPES)
    }
    fn on_statement(&self, stmt: &Statement, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        let attributes = match stmt {
            Statement::FunctionDecl(decl) => &decl.attributes,
            Statement::LocalFunction(local) => &local.attributes,
            _ => return,
        };
        // Attribute check first: the directive scan walks the comment
        // list, so only pay for it on functions that carry attributes.
        if attributes.is_empty() || !ctx.semantic.version.is_luau() || !has_native_directive(ctx) {
            return;
        }
        AttributeChecker {
            source: ctx.source,
            out,
        }
        .check_attributes(attributes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&RedundantNativeAttribute, source, LuaVersion::Luau)
    }

    fn apply(source: &str, diag: &LintDiagnostic) -> String {
        let fix = diag.fix.as_ref().expect("fix");
        let edit = &fix.edits[0];
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..edit.span.start as usize]);
        out.push_str(&edit.replacement);
        out.push_str(&source[edit.span.end as usize..]);
        let parse = luck_parser::parse(&out, LuaVersion::Luau);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
        out
    }

    #[test]
    fn flags_function_decl_attribute() {
        let source = "--!native\n@native function f() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "--!native\nfunction f() end");
    }

    #[test]
    fn flags_local_function_attribute() {
        let source = "--!native\n@native\nlocal function f() end\nf()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "--!native\nlocal function f() end\nf()");
    }

    #[test]
    fn flags_nested_function_attribute() {
        let source = "--!native\ndo\n@native function f() end\nend";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        apply(source, &diags[0]);
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
        let diags = crate::test_support::run_rule(
            &RedundantNativeAttribute,
            "--!native\nlocal _x = 1",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
