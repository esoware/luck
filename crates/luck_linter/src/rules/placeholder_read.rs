use luck_semantic::ReferenceKind;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags reads of `_`, which by convention names a value being thrown away.
///
/// A read usually means the author grabbed the wrong binding, or a nearer `_`
/// shadows the one they meant. Only a plain read counts, so not Luau's
/// `_ += 1`, which does read `_`, but is rare and clearly deliberate.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PlaceholderRead;

declare_lint_rule!(
    PlaceholderRead,
    Suspicious,
    Warning,
    "use of `_` as a value; convention reserves it for ignored bindings"
);

impl Rule for PlaceholderRead {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();
        let Some(placeholder) = tree.name_id("_") else {
            return;
        };
        for reference in tree.references() {
            if reference.name == placeholder && reference.kind == ReferenceKind::Read {
                context.diagnostic(
                    Finding::new(
                        "read of `_`, but convention is to use `_` only for ignored bindings",
                        reference.span,
                    )
                    .with_help("rename the binding or pull the value into a meaningful local"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("local _ = f()", LuaTarget::Lua54),
        ("_ = f()", LuaTarget::Lua54),
        ("_ += 1", LuaTarget::Luau),
        ("for _ = 1, 10 do end", LuaTarget::Lua54),
        ("local function f(_) end", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("print(_)", LuaTarget::Lua54),
        (
            "\
_[_] = 1
_.x = 2",
            LuaTarget::Lua54,
        ),
        // Only the read in the body fires, not the binding.
        ("for _, v in pairs(t) do print(_) end", LuaTarget::Lua54),
        ("local function f(_) return _ end", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::PlaceholderRead, PASS, FAIL).test_and_snapshot();
    }
}
