use luck_semantic::scope::{ReferenceKind, SymbolKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luacheck 213: a for-loop induction variable is declared but never
/// read inside the loop body.
///
/// Why this rule exists in addition to `unused_variable`: the catch-all
/// `unused_variable` already covers `IteratorVariable` and
/// `NumericForVariable`. Splitting it out lets users keep loud unused
/// detection elsewhere while accepting loops that only care about the
/// iteration count. Both rules firing on the same symbol is acceptable.
pub struct UnusedLoopVariable;

impl Rule for UnusedLoopVariable {
    fn name(&self) -> &'static str {
        "unused_loop_variable"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "for-loop variable is declared but never read in the loop body"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let semantic = ctx.semantic;
        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if !matches!(
                symbol.kind,
                SymbolKind::IteratorVariable | SymbolKind::NumericForVariable
            ) {
                continue;
            }
            if symbol.name == "_" || symbol.name.starts_with('_') {
                continue;
            }

            let has_read = symbol.reference_ids.iter().any(|&ref_id| {
                matches!(
                    semantic.scope_tree.reference(ref_id).kind,
                    ReferenceKind::Read | ReferenceKind::ReadWrite
                )
            });
            if has_read {
                continue;
            }

            let fix = Some(Fix {
                description: format!("rename `{}` to `_{}`", symbol.name, symbol.name),
                edits: vec![TextEdit {
                    span: symbol.definition_span,
                    replacement: format!("_{}", symbol.name),
                }],
            });

            diagnostics.push(
                LintDiagnostic::new(
                    "unused_loop_variable",
                    format!("unused loop variable `{}`", symbol.name),
                    symbol.definition_span,
                )
                .with_help("prefix with `_` to suppress this warning".to_string())
                .with_fix_opt(fix),
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
        crate::test_support::run_rule(&UnusedLoopVariable, source, LuaVersion::Lua54)
    }

    fn apply(source: &str, diag: &LintDiagnostic) -> String {
        let fix = diag.fix.as_ref().expect("fix");
        let edit = &fix.edits[0];
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..edit.span.start as usize]);
        out.push_str(&edit.replacement);
        out.push_str(&source[edit.span.end as usize..]);
        out
    }

    #[test]
    fn fix_prefixes_underscore_and_reparses() {
        let source = "for i = 1, 10 do print() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply(source, &diags[0]);
        assert_eq!(fixed, "for _i = 1, 10 do print() end");
        let parse = luck_parser::parse(&fixed, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
    }

    #[test]
    fn flags_numeric_for_unused() {
        let diags = run("for i = 1, 10 do print() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`i`"));
    }

    #[test]
    fn ignores_underscore_numeric_for() {
        let diags = run("for _ = 1, 10 do print() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_generic_for_key_only() {
        let diags = run("for k, v in pairs(t) do print(v) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`k`"));
    }

    #[test]
    fn ignores_used_numeric_for() {
        let diags = run("for i = 1, 10 do print(i) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_both_unused_generic_for() {
        let diags = run("for k, v in pairs(t) do print() end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }
}
