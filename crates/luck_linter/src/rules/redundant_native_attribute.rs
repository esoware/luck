use luck_ast::{AstKind, AstType, FunctionAttribute};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{Span, leading_hot_comments};

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RedundantNativeAttribute;

declare_lint_rule!(
    RedundantNativeAttribute,
    Style,
    Warning,
    "`@native` attribute is redundant in a `--!native` module",
    fix = Fix
);

impl Rule for RedundantNativeAttribute {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[
        AstType::FunctionDecl,
        AstType::LocalFunction,
        AstType::FunctionDef,
    ]);

    fn should_run(&self, host: &ContextHost<'_>) -> bool {
        has_native_directive(host)
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
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

fn check_attributes(attributes: &[FunctionAttribute<'_>], context: &LintContext<'_, '_>) {
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
    let after_at = attribute.span.start as usize + 1;
    let mut edit_end = attribute.name.span.end as usize;
    if bytes.get(after_at) == Some(&b'[') {
        if attribute.args.is_some()
            || attributes.iter().filter(|other| other.span.start == attribute.span.start).count()
                > 1
        {
            return None;
        }
        let whitespace_length =
            bytes[edit_end..].iter().take_while(|b| b.is_ascii_whitespace()).count();
        let close_index = edit_end + whitespace_length;
        if bytes.get(close_index) != Some(&b']') {
            return None;
        }
        edit_end = close_index + 1;
    }
    let separator_length = match &bytes[edit_end..] {
        [b'\r', b'\n', ..] => 2,
        [b' ' | b'\t' | b'\n' | b'\r', ..] => 1,
        _ => 0,
    };
    Some(Span::from_indices(attribute.span.start as usize, edit_end + separator_length))
}

/// Whether a `--!native` hot comment comes before any code. Luau ignores one
/// placed later.
fn has_native_directive(host: &ContextHost<'_>) -> bool {
    leading_hot_comments(host.comments, host.chunk.span.start)
        .any(|comment| comment.span.text(host.source)[3..].trim_ascii_end() == b"native")
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "@native function f() end",
        "\
local _x = 1
--!native
@native function f() end",
        "\
--!native
@checked function f() end",
        "\
--!native
local _x = 1",
        // The compiler ignores `--!native on`.
        "\
--!native on
@native function f() end",
    ];

    const FAIL: &[&str] = &[
        "\
--!native
@native function f() end",
        "\
--!native
@native
local function f() end
f()",
        "\
--!native
do
@native function f() end
end",
        "\
--!native
@native
local function a() end
local f = @native function() end",
        // The fix should remove the whole attribute.
        "\
--!native
@[native] function f() end",
        "\
--!native
@[native, deprecated] function f() end",
        "\
--!native 
@native function f() end",
        "\
--!native\r
@native\r
local function f() end\r
f()",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RedundantNativeAttribute, PASS, FAIL).test_and_snapshot();
    }
}
