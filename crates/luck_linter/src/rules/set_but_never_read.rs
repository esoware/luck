use luck_diagnostics::Tag;
use luck_semantic::SymbolKind;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags a local that's assigned after its declaration but never read
/// (Luacheck 231).
///
/// A lone `local x = 1` with no other references is left to
/// `unused_variable`; this rule needs at least one later write, such as
/// `x = 1`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SetButNeverRead;

declare_lint_rule!(
    SetButNeverRead,
    Style,
    Warning,
    "local is written to but its value is never read"
);

impl Rule for SetButNeverRead {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();

        for symbol in tree.symbols() {
            if symbol.kind != SymbolKind::Local {
                continue;
            }
            let name = tree.name(symbol.name);
            if name.starts_with('_') {
                continue;
            }
            // Exported bindings are read through the module's
            // export table, so no write to one is dead.
            if symbol.is_exported {
                continue;
            }

            let mut has_write = false;
            let mut has_read = false;
            for &reference_id in tree.resolved_references(symbol.id) {
                let kind = tree.reference(reference_id).kind;
                has_read |= kind.is_read();
                has_write |= kind.is_write();
            }

            if !has_write || has_read {
                continue;
            }

            context.diagnostic(
                Finding::new(
                    format!("value assigned to `{name}` is never read"),
                    symbol.definition_span,
                )
                .with_tag(Tag::Unnecessary)
                .with_help("remove the assignment or use the value somewhere"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local x
x = 1
return x",
        // No post-declaration write; that's `unused_variable`'s job.
        "local x = 1",
        "\
local _x
_x = 1",
        "\
export local counter = 0
counter = 1",
    ];

    const FAIL: &[&str] = &[
        "\
local x
x = 1",
        "\
local x
x = 1
x = 2",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::SetButNeverRead, PASS, FAIL).test_and_snapshot();
    }
}
