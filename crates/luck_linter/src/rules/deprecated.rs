use luck_ast::{AstKind, AstType, Call, GetSpan as _, is_valid_identifier};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{ResolvedCallee, SemanticAnalysis};

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::{Fix, FixKind, RuleFixer};
use crate::rule::{Rule, declare_lint_rule};

/// A deprecation whose `use` names what to call instead puts it in the
/// callee's place as a suggestion, since a definition can't say the
/// replacement behaves the same.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Deprecated;

declare_lint_rule!(
    Deprecated,
    Correctness,
    Warning,
    "use of a deprecated standard library function",
    fix = Suggestion
);

impl Rule for Deprecated {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (index, call) in chain.calls() {
            if let Some(callee) = context.callee(node, index) {
                check_call(call, &callee, context);
            }
        }
    }
}

fn check_call(call: Call<'_>, callee: &ResolvedCallee<'_>, context: &LintContext<'_, '_>) {
    let Some(deprecation) = callee.entry().deprecation() else {
        return;
    };
    let name = callee.name();
    let finding =
        Finding::new(format!("`{name}` is deprecated"), call.span()).with_tag(Tag::Deprecated);
    let finding = match deprecation.advice() {
        Some(advice) => finding.with_help(advice.into_owned()),
        None => finding,
    };
    context.diagnostic(match deprecation.r#use {
        Some(replacement) => finding.with_fix(context, FixKind::Suggestion, |fixer| {
            replacement_fix(fixer, call, &name, replacement, context)
        }),
        None => finding,
    });
}

/// `replacement` in the callee's place: a method's name for a method call,
/// or a dotted path of names for any other, or `None` when it's neither, or
/// when its first name may not be the host's global there.
fn replacement_fix(
    fixer: RuleFixer,
    call: Call<'_>,
    name: &str,
    replacement: &str,
    context: &ContextHost<'_>,
) -> Option<Fix> {
    let span = if let Some(method) = call.method {
        if !is_valid_identifier(replacement) {
            return None;
        }
        method.span
    } else {
        if !replacement.split('.').all(is_valid_identifier)
            || is_rebound(replacement, call.span().start, context.semantic)
        {
            return None;
        }
        call.callee.span()
    };
    let message = format!("replace deprecated `{name}` with `{replacement}`");
    Some(fixer.replace(message, span, replacement.as_bytes().to_vec()))
}

/// Whether the name `path` starts with may not be the host's global at
/// `offset`, because a local before it or a write in the chunk rebinds it.
fn is_rebound(path: &str, offset: u32, semantic: &SemanticAnalysis<'_>) -> bool {
    let name = path.split('.').next().unwrap_or(path);
    let tree = semantic.scoping();
    tree.nearest_local(name, offset).is_some()
        || tree.name_id(name).is_some_and(|id| tree.is_assigned_global(id))
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        // The type of `obj` is unknown, so `getn` doesn't resolve to a stdlib
        // entry.
        "obj:getn()",
        "local n = #t",
    ];

    const FAIL: &[&str] = &[
        // Luau's library reference says what to do instead, but names no
        // function.
        "local n = table.getn(t)",
        "table.foreach(t, print)",
        "table.foreachi(t, print)",
    ];

    const ROBLOX_PASS: &[&str] = &[
        "local p = game:GetService('Players')",
        "local p = Instance.new('Part', workspace)",
        "task.wait(1)",
    ];

    const ROBLOX_FAIL: &[&str] = &[
        "wait(1)",
        "delay(1, f, 'x')",
        "local thread = spawn(f)",
        // `task.wait` would read the local `task`, so there's no fix.
        "\
local task = {}
wait(1)",
        "ypcall(f)",
        "local s = stats()",
        "local n = collectgarbage('count')",
        "local children = script:children()",
        "local child = workspace:findFirstChild('x')",
    ];

    const DEFINITIONS: &str = r#"
declare app: { start: () -> () }
@[deprecated {use = "app.start", reason = "begin was renamed"}] declare function begin()
@[deprecated {use = "the start function"}] declare function launch()
"#;

    const DEFINITIONS_FAIL: &[&str] = &["begin()", "launch()"];

    #[test]
    fn test() {
        Tester::new(RuleId::Deprecated, PASS, FAIL)
            .with_roblox(ROBLOX_PASS, ROBLOX_FAIL)
            .with_definitions(DEFINITIONS, &[], DEFINITIONS_FAIL)
            .test_and_snapshot();
    }
}
