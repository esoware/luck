use luck_diagnostics::Tag;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags a value written to a local that some path overwrites and no path
/// reads first (Luacheck 311).
///
/// A captured local is skipped, since a closure can read it at any time.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ValueOverwrittenBeforeRead;

declare_lint_rule!(
    ValueOverwrittenBeforeRead,
    Correctness,
    Warning,
    "value assigned to a local is overwritten before it is read"
);

impl Rule for ValueOverwrittenBeforeRead {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();
        for &(symbol_id, span) in &context.local_flow().overwritten_values {
            context.diagnostic(
                Finding::new(
                    format!(
                        "value assigned to `{}` is overwritten before it is read",
                        tree.name(tree.symbol(symbol_id).name)
                    ),
                    span,
                )
                .with_tag(Tag::Unnecessary)
                .with_help("remove the earlier assignment or read it first"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        // Writes on separate branches don't overwrite each other.
        "\
local x = 1
if c then x = 2 end
return x",
        // `local a, b = 1` assigns `b` no value.
        "\
local a, b = 1
b = 2
return a, b",
        "\
local x = 1
print(x)
x = 2
return x",
        "\
local x = 1
return x",
        "\
local _x = 1
_x = 2
return _x",
        // The right-hand side reads the old value.
        "\
local x = 1
x = x + 1
return x",
        // The `continue` path reads the initializer.
        "\
local x = 1
repeat
  if c then continue end
  x = 2
until true
print(x)",
        "\
local x = 1
for _ = 1, 3 do print(x)
x = 2 end",
    ];

    const FAIL: &[&str] = &[
        "\
local x = 1
x = 2
return x",
        // Write then write with no read between, so the first is dead.
        "\
local x
x = 1
x = 2
return x",
        "\
local x = 1
if c then x = 2 else x = 3 end
return x",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::ValueOverwrittenBeforeRead, PASS, FAIL).test_and_snapshot();
    }
}
