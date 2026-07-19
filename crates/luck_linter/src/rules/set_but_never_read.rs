use luck_semantic::scope::{ReferenceKind, SymbolKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luacheck 231: a local is assigned to after its declaration but its
/// value is never read.
///
/// Why this is distinct from `unused_variable`: `unused_variable` fires
/// when the declaration itself is unused, which is enough for
/// `local x = 1`. This rule targets the case where the user *does*
/// reach for the variable later (`x = 1`) but no code ever reads it.
/// The signal is "the assignment is dead", not "the declaration is
/// dead". We avoid overlap by requiring at least one post-declaration
/// Write - `local x = 1` with no further references is left to
/// `unused_variable`.
pub struct SetButNeverRead;

impl Rule for SetButNeverRead {
    fn name(&self) -> &'static str {
        "set_but_never_read"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "local is written to but its value is never read"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let _block = ctx.block;
        let semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.kind != SymbolKind::Local {
                continue;
            }
            if symbol.name == "_" || symbol.name.starts_with('_') {
                continue;
            }

            let mut has_write = false;
            let mut has_read = false;
            for &ref_id in &symbol.reference_ids {
                match semantic.scope_tree.references[ref_id.index()].kind {
                    ReferenceKind::Read => has_read = true,
                    ReferenceKind::Write => has_write = true,
                    ReferenceKind::ReadWrite => {
                        has_read = true;
                        has_write = true;
                    }
                }
            }

            if !has_write || has_read {
                continue;
            }

            diagnostics.push(
                LintDiagnostic::new(
                    "set_but_never_read",
                    format!("value assigned to '{}' is never read", symbol.name),
                    symbol.definition_span,
                )
                .with_help("remove the assignment or use the value somewhere".to_string()),
            );
        }

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&SetButNeverRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_write_without_read() {
        let diags = run("local x\nx = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'x'"));
    }

    #[test]
    fn ignores_write_with_subsequent_read() {
        let diags = run("local x\nx = 1\nreturn x");
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
        let diags = run("local _x\n_x = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_multiple_writes_no_reads() {
        let diags = run("local x\nx = 1\nx = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }
}
