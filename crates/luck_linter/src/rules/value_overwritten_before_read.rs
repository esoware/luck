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
    fn run_once(&self, context: &LintContext<'_>) {
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
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::ValueOverwrittenBeforeRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_initializer_then_overwrite() {
        let diags = run("\
local x = 1
x = 2
return x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_branched_overwrite() {
        let diags = run("\
local x = 1
if c then x = 2 end
return x");
        assert!(diags.is_empty(), "branched writes must not fire (conservative): {diags:?}");
    }

    #[test]
    fn ignores_name_the_initializer_list_leaves_without_a_value() {
        let diags = run("\
local a, b = 1
b = 2
return a, b");
        assert!(diags.is_empty(), "`local a, b = 1` assigns `b` no value: {diags:?}");
    }

    #[test]
    fn ignores_read_between_writes() {
        let diags = run("\
local x = 1
print(x)
x = 2
return x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_single_initializer() {
        let diags = run("\
local x = 1
return x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_two_writes_after_declaration() {
        // Write then write with no read between, so the first is dead.
        let diags = run("\
local x
x = 1
x = 2
return x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("\
local _x = 1
_x = 2
return _x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_assignment_reading_its_own_target() {
        let diags = run("\
local x = 1
x = x + 1
return x");
        assert!(diags.is_empty(), "the right-hand side reads the old value: {diags:?}");
    }

    #[test]
    fn ignores_write_skipped_by_goto() {
        let diags = run("\
local x = 1
if c then goto skip end
x = 2
::skip::
print(x)");
        assert!(diags.is_empty(), "the `goto` path reads the initializer: {diags:?}");
    }

    #[test]
    fn flags_the_overwritten_assignment_at_its_name() {
        let source = "\
local x
x = 1
x = 2
return x";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].span().text(source.as_bytes()),
            b"x",
            "the span should cover `x`: {diags:?}"
        );
    }

    #[test]
    fn flags_initializer_overwritten_on_every_branch() {
        let diags = run("\
local x = 1
if c then x = 2 else x = 3 end
return x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_value_read_in_the_next_iteration() {
        let diags = run("\
local x = 1
for _ = 1, 3 do print(x)
x = 2 end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
