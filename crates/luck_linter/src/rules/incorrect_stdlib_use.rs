use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{ResolvedCallee, StdlibEntry, StdlibFunction};

use crate::call::{CallArg, CallArgs};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// How many allowed values a message lists, since a definition's union of
/// strings can run to hundreds.
const LISTED_VALUE_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct IncorrectStdlibUse;

declare_lint_rule!(
    IncorrectStdlibUse,
    Correctness,
    Error,
    "wrong argument count or invalid constant in a standard library call"
);

impl Rule for IncorrectStdlibUse {
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
    let StdlibEntry::Function(function) = callee.entry() else {
        return;
    };
    let args = CallArgs::new(call.args);
    let arg_count = args.len();
    // A trailing call or `...` can expand to any number of values, so the
    // minimum and overload checks skip it.
    let has_multi_value_tail = args.clone().last().is_some_and(CallArg::is_multi_value);

    let min_args = function.min_args();
    let arity_message = if arg_count < min_args && !has_multi_value_tail {
        Some(format!(
            "`{}` requires at least {min_args} argument(s), got {arg_count}",
            callee.name()
        ))
    } else if let Some(max_args) = function.max_args()
        && arg_count > max_args
    {
        Some(format!("`{}` accepts at most {max_args} argument(s), got {arg_count}", callee.name()))
    } else if !function.accepts_arg_count(arg_count) && !has_multi_value_tail {
        // Within the overall bounds but between overloads, like 1 argument
        // to a function that takes 0 or 2.
        Some(format!("no overload of `{}` accepts {arg_count} argument(s)", callee.name()))
    } else {
        None
    };
    match arity_message {
        Some(message) => context.diagnostic(Finding::new(message, call.span())),
        None => {
            for finding in invalid_constant_args(callee, function, args, arg_count) {
                context.diagnostic(finding);
            }
        }
    }
}

/// With overloads, a value passes if any signature matching the arg count
/// allows it, and a position some matching signature leaves unconstrained
/// isn't checked.
fn invalid_constant_args(
    callee: &ResolvedCallee<'_>,
    function: &StdlibFunction<'_>,
    args: CallArgs<'_>,
    arg_count: usize,
) -> impl Iterator<Item = Finding> {
    args.enumerate().filter_map(move |(index, arg)| {
        let (value, span) = arg.string_literal_value()?;
        let allowed_sets = function.allowed_constants(index, arg_count)?;
        let allowed = || allowed_sets.clone().flatten();
        if allowed().any(|constant| *constant == value) {
            return None;
        }
        let allowed_list = allowed()
            .take(LISTED_VALUE_LIMIT)
            .map(|constant| format!("`{constant}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let allowed_count = allowed().count();
        let truncation_note = if allowed_count > LISTED_VALUE_LIMIT {
            format!(", ... ({allowed_count} allowed values)")
        } else {
            String::new()
        };
        let arg_number = index + 1;
        Some(Finding::new(
            format!(
                "`{}` argument {arg_number} must be one of \
                 {allowed_list}{truncation_note}, got `{value}`",
                callee.name(),
            ),
            span,
        ))
    })
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "table.insert(t, 1)",
        // `obj` has no known shape, so `foo` doesn't resolve to the stdlib.
        "obj:foo()",
        "\
local type = f
type(1, 2, 3)",
        // One overload of `os.date` takes any string.
        "local now = os.date('%c')",
    ];

    const FAIL: &[&str] = &[
        "table.insert()",
        "type(1, 2, 3)",
        "local s = ('x'):rep()",
        // Luau's definitions take a message, as its checker says.
        "error()",
        // `table.insert` takes two or three, as its overloads do.
        "table.insert(t, 1, 2, 3)",
    ];

    const ROBLOX_PASS: &[&str] = &[
        // Every `CFrame.new` overload: no arguments, one or two `Vector3`s, and 3,
        // 7, or 12 numbers.
        "\
local v = Vector3.new(1, 2, 3)
local a = CFrame.new()
local b = CFrame.new(v)
local c = CFrame.new(v, v)
local d = CFrame.new(1, 2, 3)
local e = CFrame.new(1, 2, 3, 4, 5, 6, 7)
local f = CFrame.new(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)",
        "\
local u = UDim.new(0, 10)
local a = UDim2.new(0, 0, 1, 0)
local b = UDim2.new(u, u)",
        "game:GetService('ProximityPromptService')",
        "local x = script:IsA('BasePart')",
        "Instance.new('Folder')",
        "BrickColor.new('Bright red')",
        "BrickColor.new(1, 0, 0)",
    ];

    const ROBLOX_FAIL: &[&str] = &[
        // Five arguments match no `CFrame.new` overload.
        "local c = CFrame.new(1, 2, 3, 4, 5)",
        // A local holds the shape of the call it's set to.
        "\
local child = script:FindFirstChild('x')
local grandchild = child:FindFirstChild()",
        "game:GetService()",
        "BrickColor.new(1, 0)",
    ];

    const DEFINITIONS: &str = r#"
type Mode = "r" | "w"
declare fs: { open: (path: string, mode: Mode?) -> () }
"#;

    const DEFINITIONS_PASS: &[&str] = &["fs.open('a', 'r')", "fs.open('a')", "fs.open('a', mode)"];

    const DEFINITIONS_FAIL: &[&str] = &["fs.open('a', 'x')", "fs.open()"];

    #[test]
    fn test() {
        Tester::new(RuleId::IncorrectStdlibUse, PASS, FAIL)
            .with_roblox(ROBLOX_PASS, ROBLOX_FAIL)
            .with_definitions(DEFINITIONS, DEFINITIONS_PASS, DEFINITIONS_FAIL)
            .test_and_snapshot();
    }
}
