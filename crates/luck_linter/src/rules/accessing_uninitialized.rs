use luck_ast::Statement;
use luck_ast::visitor::Visitor;
use luck_semantic::scope::{ReferenceKind, SymbolKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luacheck 321: a local is declared without an initializer, then read
/// before any value is assigned to it. Lua semantics make the read
/// produce `nil` - almost always a bug.
///
/// Why this is distinct from `unused_variable`: the local *is* used,
/// it's just used too early. We require the declaration to have no RHS
/// (`local x` rather than `local x = nil` or `local x = ...`) so that
/// `redundant_nil_init` and this rule are orthogonal.
pub struct AccessingUninitialized;

impl Rule for AccessingUninitialized {
    fn name(&self) -> &'static str {
        "accessing_uninitialized"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "local is read before any value is assigned to it"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut uninit = UninitCollector::default();
        uninit.visit_block(block);

        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.kind != SymbolKind::Local {
                continue;
            }
            if !uninit.is_uninit(symbol.definition_span.start, symbol.definition_span.end) {
                continue;
            }
            if symbol.name == "_" || symbol.name.starts_with('_') {
                continue;
            }

            // First reference ANYWHERE, by source order. Filtering to the
            // declaration scope made writes inside `if`/loop bodies
            // invisible and flagged the ubiquitous branch-initialization
            // idiom (`local x if c then x = 1 end print(x)`).
            let mut refs: Vec<_> = symbol
                .reference_ids
                .iter()
                .map(|&ref_id| &semantic.scope_tree.references[ref_id.index()])
                .collect();
            refs.sort_by_key(|r| r.span.start);

            let Some(first) = refs.first() else {
                continue;
            };
            if !matches!(first.kind, ReferenceKind::Read | ReferenceKind::ReadWrite) {
                continue;
            }

            diagnostics.push(
                LintDiagnostic::new(
                    "accessing_uninitialized",
                    format!(
                        "'{}' is read before any value is assigned; result is nil",
                        symbol.name
                    ),
                    first.span,
                )
                .with_help("initialize at declaration or assign before reading".to_string()),
            );
        }

        diagnostics
    }
}

/// AST pass that records the definition spans of locals declared
/// without an initializer.
#[derive(Default)]
struct UninitCollector {
    spans: Vec<(u32, u32)>,
}

impl UninitCollector {
    fn is_uninit(&self, start: u32, end: u32) -> bool {
        self.spans.iter().any(|(s, e)| *s == start && *e == end)
    }
}

impl Visitor for UninitCollector {
    fn visit_statement(&mut self, stmt: &Statement) {
        if let Statement::LocalAssignment(local) = stmt
            && local.equal_and_exprs.is_none()
        {
            for attributed in local.names.iter() {
                self.spans
                    .push((attributed.name.span.start, attributed.name.span.end));
            }
        }
        self.walk_statement(stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&AccessingUninitialized, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_read_before_assign() {
        let diags = run("local x\nprint(x)");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'x'"));
    }

    #[test]
    fn ignores_explicit_nil_init() {
        // `local x = nil` is explicit; `redundant_nil_init` may comment
        // on the style choice, but this rule treats it as initialized.
        let diags = run("local x = nil\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_write_then_read() {
        let diags = run("local x\nx = 1\nprint(x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x\nprint(_x)");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
