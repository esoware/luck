use luck_semantic::SymbolKind;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags a local that's assigned after its declaration but never read
/// (Luacheck 231).
///
/// A lone `local x = 1` with no other references is left to
/// `unused_variable`; this rule needs at least one later write, such as
/// `x = 1`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SetButNeverRead;

impl_rule!(
    SetButNeverRead,
    "set_but_never_read",
    Style,
    Warning,
    "local is written to but its value is never read"
);

impl WholeRule for SetButNeverRead {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let mut findings = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.kind != SymbolKind::Local {
                continue;
            }
            if symbol.name.starts_with('_') {
                continue;
            }
            // Luau: exported bindings are read through the module's
            // export table, so no write to one is dead.
            if symbol.is_exported {
                continue;
            }

            let mut has_write = false;
            let mut has_read = false;
            for &reference_id in &symbol.reference_ids {
                let kind = semantic.scope_tree.reference(reference_id).kind;
                has_read |= kind.is_read();
                has_write |= kind.is_write();
            }

            if !has_write || has_read {
                continue;
            }

            findings.push(
                Finding::new(
                    format!("value assigned to `{}` is never read", symbol.name),
                    symbol.definition_span,
                )
                .with_help("remove the assignment or use the value somewhere"),
            );
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&SetButNeverRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_write_without_read() {
        let diags = run("\
local x
x = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_write_with_subsequent_read() {
        let diags = run("\
local x
x = 1
return x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_pure_unused_declaration() {
        // No post-declaration write; that's `unused_variable`'s job.
        let diags = run("local x = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("\
local _x
_x = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_multiple_writes_no_reads() {
        let diags = run("\
local x
x = 1
x = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_exported_local_only_written() {
        let diags = run_rule(
            &SetButNeverRead,
            "\
export local counter = 0
counter = 1",
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
