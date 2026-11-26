use luck_ast::{AstKind, AstType, Call, GetSpan as _, is_valid_identifier};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{ResolvedCallee, SemanticAnalysis, StdlibReplacement};

use crate::call::{CallArg, CallArgs};
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::{Fix, FixKind, RuleFixer};
use crate::rule::{Rule, declare_lint_rule};

/// A deprecation whose `use` names what to call instead puts it in the
/// callee's place as a suggestion, since a definition can't say the
/// replacement behaves the same, but only in a call that the types the
/// definition file declares for both say the replacement takes, and whose
/// values the code drops unless it returns the same.
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
        let dropped = context.dropped_call(node);
        for (index, call) in chain.calls() {
            if let Some(callee) = context.callee(node, index) {
                check_call(call, &callee, dropped != Some(index), context);
            }
        }
    }
}

fn check_call(
    call: Call<'_>,
    callee: &ResolvedCallee<'_>,
    is_value_used: bool,
    context: &LintContext<'_, '_>,
) {
    let Some(deprecation) = callee.entry().deprecation() else {
        return;
    };
    let name = callee.name();
    let mut finding =
        Finding::new(format!("`{name}` is deprecated"), call.span()).with_tag(Tag::Deprecated);
    if let Some(replacement) = deprecation.r#use {
        finding = finding.with_help(format!("use `{replacement}` instead"));
    }
    if let Some(reason) = deprecation.reason {
        finding = finding.with_note(reason.to_owned());
    }
    let takes_call = deprecation
        .replacement
        .is_some_and(|replacement| takes_call(&replacement, call, is_value_used));
    context.diagnostic(match deprecation.r#use.filter(|_| takes_call) {
        Some(replacement) => finding.with_fix(context, FixKind::Suggestion, |fixer| {
            replacement_fix(fixer, call, &name, replacement, context)
        }),
        None => finding,
    });
}

/// Whether `replacement` takes `call` in the deprecated function's place: a
/// call of its form, a method's or not, of as many arguments as it takes,
/// which no last argument can make more or fewer, and whose values the code
/// drops unless `replacement` returns the same.
fn takes_call(replacement: &StdlibReplacement, call: Call<'_>, is_value_used: bool) -> bool {
    let args = CallArgs::new(call.args);
    replacement.is_method == call.method.is_some()
        && !args.clone().last().is_some_and(CallArg::is_multi_value)
        && replacement.takes(args.len(), is_value_used)
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
        // `task.wait` returns one number, where `wait` returns two.
        "local elapsed = wait(1)",
        // The last argument can be any number of values.
        "wait(duration())",
        "delay(1, f)",
        // `delay` takes two arguments at most.
        "delay(1, f, 'x')",
        "spawn(f)",
        // `task.spawn` returns a thread, where `spawn` returns nothing.
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
        // `GetPartBoundsInBox` takes a `CFrame` and a size, and returns a
        // table, which is always truthy.
        "if workspace:IsRegion3Empty(region) then end",
        "workspace:FindPartsInRegion3(region, ignore, 10)",
        // `Raycast` takes an origin and a direction.
        "local hit = workspace:FindPartOnRay(ray)",
    ];

    const DEFINITIONS: &str = r#"
declare app: { start: () -> (), stop: (code: number) -> (), open: (path: string) -> () }
@[deprecated {use = "app.start", reason = "begin was renamed"}] declare function begin()
@[deprecated {use = "the start function"}] declare function launch()
@[deprecated {use = "app.stop"}] declare function halt()
@[deprecated {use = "app.open"}] declare function load(path: number)
@[deprecated {use = "print"}] declare function log(...: any)
declare extern type Door with
    @[deprecated {use = "Open"}] function open(self): ()
    function Open(self): ()
end
declare door: Door
"#;

    const DEFINITIONS_FAIL: &[&str] = &[
        "begin()",
        "launch()",
        // `app.stop` needs a code.
        "halt()",
        // `app.open` takes a string.
        "load(1)",
        // The file that deprecates `log` doesn't declare `print`, so it can't
        // say what `print` takes.
        "log('x')",
        "door:open()",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::Deprecated, PASS, FAIL)
            .with_roblox(ROBLOX_PASS, ROBLOX_FAIL)
            .with_definitions(DEFINITIONS, &[], DEFINITIONS_FAIL)
            .test_and_snapshot();
    }
}
