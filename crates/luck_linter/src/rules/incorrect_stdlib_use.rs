use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{ResolvedCallee, StdlibEntry, StdlibFunction};

use crate::call::{CallArg, CallArgs, calls_in, string_literal_value};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// How many allowed values a message lists. Generated sets (Roblox service
/// and class names) run to hundreds of values.
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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (index, call) in calls_in(chain) {
            if let Some(callee) = context.callee(node, index) {
                check_call(call, &callee, context);
            }
        }
    }
}

fn check_call(call: Call<'_>, callee: &ResolvedCallee<'_>, context: &LintContext<'_>) {
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
        let (value, span) = string_literal_value(arg)?;
        let allowed_sets = function.allowed_constants(index, arg_count)?;
        let allowed = || allowed_sets.clone().flatten();
        if allowed().any(|constant| constant.value == value) {
            return None;
        }
        let allowed_list = allowed()
            .take(LISTED_VALUE_LIMIT)
            .map(|constant| format!("`{}`", constant.value))
            .collect::<Vec<_>>()
            .join(", ");
        let allowed_count = allowed().count();
        let truncation_note = if allowed_count > LISTED_VALUE_LIMIT {
            format!(", ... ({allowed_count} allowed values)")
        } else {
            String::new()
        };
        let arg_number = index.saturating_add(1);
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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("table.insert(t, 1)", LuaTarget::Lua54),
        ("collectgarbage('collect')", LuaTarget::Lua54),
        // `obj` has no known shape, so `foo` doesn't resolve to the stdlib.
        ("obj:foo()", LuaTarget::Lua54),
        // Every `CFrame.new` overload: no arguments, one or two `Vector3`s, and 3,
        // 7, or 12 numbers.
        (
            "\
local v = Vector3.new(1, 2, 3)
local a = CFrame.new()
local b = CFrame.new(v)
local c = CFrame.new(v, v)
local d = CFrame.new(1, 2, 3)
local e = CFrame.new(1, 2, 3, 4, 5, 6, 7)
local f = CFrame.new(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)",
            LuaTarget::LuauRoblox,
        ),
        (
            "\
local u = UDim.new(0, 10)
local a = UDim2.new(0, 0, 1, 0)
local b = UDim2.new(u, u)",
            LuaTarget::LuauRoblox,
        ),
        // The 5.5 three-arg tuning form matches only the `param` overload; the
        // plain-option overloads must not reject it.
        ("collectgarbage('param', 'pause', 100)", LuaTarget::Lua55),
        ("collectgarbage('incremental', 100, 200, 13)", LuaTarget::Lua54),
        (
            "\
local type = f
type(1, 2, 3)",
            LuaTarget::Lua54,
        ),
        (
            "\
local f = something()
f:setvbuf()",
            LuaTarget::Lua54,
        ),
        ("game:GetService('ProximityPromptService')", LuaTarget::LuauRoblox),
        ("local x = script:IsA('BasePart')", LuaTarget::LuauRoblox),
        ("Instance.new('Folder')", LuaTarget::LuauRoblox),
        ("BrickColor.new('Bright red')", LuaTarget::LuauRoblox),
        // The three-number form skips the name check.
        ("BrickColor.new(1, 0, 0)", LuaTarget::LuauRoblox),
        // `error()` raises a nil error.
        ("error()", LuaTarget::Lua54),
        // `os.setlocale()` queries the locale.
        ("local l = os.setlocale()", LuaTarget::Lua54),
        // Standalone Luau accepts `collect`.
        ("collectgarbage('collect')", LuaTarget::Luau),
        // Lua 5.2 accepts `setmajorinc`.
        (r#"local n = collectgarbage("setmajorinc", 100)"#, LuaTarget::Lua52),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("table.insert()", LuaTarget::Lua54),
        ("type(1, 2, 3)", LuaTarget::Lua54),
        // `collectgarbage` first arg must be one of the option names.
        ("collectgarbage('frob')", LuaTarget::Lua54),
        ("collectgarbage'frob'", LuaTarget::Lua54),
        // Five arguments match no `CFrame.new` overload.
        ("local c = CFrame.new(1, 2, 3, 4, 5)", LuaTarget::LuauRoblox),
        (
            "\
local f = io.open('x')
f:setvbuf()",
            LuaTarget::Lua54,
        ),
        (
            "\
local f = io.open('x')
f:seek('nope')",
            LuaTarget::Lua54,
        ),
        ("local s = ('x'):rep()", LuaTarget::Lua54),
        ("game:GetService('Playerz')", LuaTarget::LuauRoblox),
        ("game:GetService'Playerz'", LuaTarget::LuauRoblox),
        ("Instance.new('Playerz')", LuaTarget::LuauRoblox),
        // `BasePart` exists but isn't creatable, so `IsA` accepts it and
        // `Instance.new` doesn't.
        ("Instance.new('BasePart')", LuaTarget::LuauRoblox),
        ("game:GetService()", LuaTarget::LuauRoblox),
        ("BrickColor.new('Bright rad')", LuaTarget::LuauRoblox),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::IncorrectStdlibUse, PASS, FAIL).test_and_snapshot();
    }
}
