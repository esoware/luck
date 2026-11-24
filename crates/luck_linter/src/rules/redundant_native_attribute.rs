use luck_ast::{AstKind, AstType, FunctionAttribute};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{Span, leading_hot_comments, offset_to_index};

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RedundantNativeAttribute;

impl_rule!(
    RedundantNativeAttribute,
    "redundant_native_attribute",
    Style,
    Warning,
    "`@native` attribute is redundant in a `--!native` module",
    Fix
);

impl RuleRunner for RedundantNativeAttribute {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[
            AstType::FunctionDecl,
            AstType::LocalFunction,
            AstType::FunctionDef,
        ])
    }

    fn should_run(&self, host: &ContextHost<'_>) -> bool {
        has_native_directive(host)
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let kind = node.kind();
        let attributes = if let AstKind::FunctionDecl(function_decl) = kind {
            &function_decl.attributes
        } else if let AstKind::LocalFunction(local_function) = kind {
            &local_function.attributes
        } else if let AstKind::FunctionDef(function) = kind {
            &function.attributes
        } else {
            return;
        };
        check_attributes(attributes, context);
    }
}

fn check_attributes(attributes: &[FunctionAttribute<'_>], context: &LintContext<'_>) {
    for attribute in attributes {
        if attribute.name.text != "native" {
            continue;
        }
        context.diagnostic(
            Finding::new(
                "`@native` attribute is redundant in a `--!native` module",
                attribute.span,
            )
            .with_tag(Tag::Unnecessary)
            .with_fix(context, FixKind::Fix, |fixer| {
                let span = removal_span(attribute, attributes, context.source)?;
                Some(fixer.delete("remove the redundant `@native` attribute", span))
            }),
        );
    }
}

/// `attribute` and the space or line break after it, so no doubled separator
/// is left behind. A `@[...]` list is removed only when `attribute` is its
/// sole entry and nothing but its name sits in brackets.
fn removal_span(
    attribute: &FunctionAttribute<'_>,
    attributes: &[FunctionAttribute<'_>],
    bytes: &[u8],
) -> Option<Span> {
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
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        let close_index = edit_end.saturating_add(whitespace_length);
        if bytes.get(close_index) != Some(&b']') {
            return None;
        }
        edit_end = close_index.saturating_add(1);
    }
    let separator_length = match bytes.get(edit_end..).unwrap_or_default() {
        [b'\r', b'\n', ..] => 2,
        [b' ' | b'\t' | b'\n' | b'\r', ..] => 1,
        _ => 0,
    };
    Some(Span::from_indices(
        offset_to_index(attribute.span.start),
        edit_end.saturating_add(separator_length),
    ))
}

/// Whether a `--!native` hot comment comes before any code. Luau ignores one
/// placed later.
fn has_native_directive(host: &ContextHost<'_>) -> bool {
    leading_hot_comments(host.comments, host.chunk.block.span.start).any(|comment| {
        comment
            .span
            .text(host.source)
            .get(3..)
            .is_some_and(|word| word.trim_ascii_end() == b"native")
    })
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<Message> {
        run_rule(&RedundantNativeAttribute, source, LuaVersion::Luau)
    }

    #[test]
    fn flags_function_decl_attribute() {
        let source = "\
--!native
@native function f() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(
            fixed,
            "\
--!native
function f() end"
        );
    }

    #[test]
    fn flags_local_function_attribute() {
        let source = "\
--!native
@native
local function f() end
f()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(
            fixed,
            "\
--!native
local function f() end
f()"
        );
    }

    #[test]
    fn flags_nested_function_attribute() {
        let source = "\
--!native
do
@native function f() end
end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        fix_and_reparse(source, &diags, LuaVersion::Luau);
    }

    #[test]
    fn flags_function_expr_attribute() {
        let source = "\
--!native
@native
local function a() end
local f = @native function() end";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(
            fixed,
            "\
--!native
local function a() end
local f = function() end"
        );
    }

    #[test]
    fn ignores_without_native_directive() {
        let diags = run("@native function f() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_misplaced_native_directive() {
        let diags = run("\
local _x = 1
--!native
@native function f() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_other_attributes() {
        let diags = run("\
--!native
@checked function f() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_luau() {
        let diags = run_rule(
            &RedundantNativeAttribute,
            "\
--!native
local _x = 1",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_bracketed_attribute() {
        let source = "\
--!native
@[native] function f() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(
            fixed,
            "\
--!native
function f() end",
            "the fix should remove the whole attribute"
        );
    }

    #[test]
    fn ignores_native_directive_with_trailing_text() {
        let diags = run("\
--!native on
@native function f() end");
        assert!(diags.is_empty(), "the compiler ignores `--!native on`: {diags:?}");
    }

    #[test]
    fn flags_without_fix_when_attribute_shares_a_list() {
        let diags = run("\
--!native
@[native, deprecated] function f() end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn flags_with_directive_followed_by_trailing_space() {
        let diags = run("--!native \n@native function f() end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_removing_a_whole_crlf_line_break() {
        let source = "--!native\r\n@native\r\nlocal function f() end\r\nf()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "--!native\r\nlocal function f() end\r\nf()");
    }
}
