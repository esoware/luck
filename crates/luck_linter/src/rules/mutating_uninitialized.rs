use luck_ast::expr::Var;
use luck_ast::visitor::Visitor;
use luck_ast::{Expression, Statement};
use luck_semantic::scope::{ReferenceKind, SymbolKind};
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luacheck 341: a local is declared without an initializer, then a
/// field or index assignment is performed on it before any value is
/// assigned to the variable itself. `local x; x.foo = 1` is a runtime
/// error - `x` is `nil`.
///
/// Why a custom AST pass: the scope builder records `x.foo = ...` as a
/// *read* of `x` (it has to evaluate the prefix to do the field
/// assignment). We need to disambiguate that read from `print(x)` -
/// only the former is a field/index write target.
pub struct MutatingUninitialized;

impl Rule for MutatingUninitialized {
    fn name(&self) -> &'static str {
        "mutating_uninitialized"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "field or index assignment on an uninitialized local"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut collector = MutationCollector::default();
        collector.visit_block(block);

        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.kind != SymbolKind::Local {
                continue;
            }
            if !collector.is_uninit(symbol.definition_span.start, symbol.definition_span.end) {
                continue;
            }
            if symbol.name == "_" || symbol.name.starts_with('_') {
                continue;
            }

            // First reference ANYWHERE, by source order - writes inside
            // nested branch/loop scopes initialize too (see
            // accessing_uninitialized for the idiom this protects).
            let mut refs: Vec<_> = symbol
                .reference_ids
                .iter()
                .map(|&ref_id| &semantic.scope_tree.references[ref_id.index()])
                .collect();
            refs.sort_by_key(|r| r.span.start);

            let Some(first) = refs.first() else {
                continue;
            };
            // Only field/index writes on this name qualify. The first
            // reference must be the prefix of such a write.
            if !collector.is_field_write_prefix(first.span.start) {
                continue;
            }
            // A bare-name write would have shown up as Write kind. Reads
            // from `x.foo` (rvalue) also count as Read but live in an
            // Expression position, not an Assignment target - those are
            // not recorded by `field_write_prefix_positions`.
            if !matches!(first.kind, ReferenceKind::Read) {
                continue;
            }

            diagnostics.push(
                LintDiagnostic::new(
                    "mutating_uninitialized",
                    format!(
                        "field/index assignment on uninitialized '{}' (value is nil)",
                        symbol.name
                    ),
                    first.span,
                )
                .with_help("initialize the local before mutating its fields".to_string()),
            );
        }

        diagnostics
    }
}

/// AST pass that records:
/// (1) definition spans of locals declared without an initializer, and
/// (2) source positions of bare names used as the prefix of a field or
///     index write target (`<name>.foo = ...` or `<name>[k] = ...`).
#[derive(Default)]
struct MutationCollector {
    uninit_spans: Vec<(u32, u32)>,
    field_write_prefix_positions: Vec<u32>,
}

impl MutationCollector {
    fn is_uninit(&self, start: u32, end: u32) -> bool {
        self.uninit_spans
            .iter()
            .any(|(s, e)| *s == start && *e == end)
    }

    fn is_field_write_prefix(&self, position: u32) -> bool {
        self.field_write_prefix_positions.contains(&position)
    }

    fn record_target(&mut self, var: &Var) {
        match var {
            Var::Name(_) => {}
            Var::FieldAccess(fa) => {
                if let Expression::Var(prefix_var) = &fa.prefix
                    && let Var::Name(token) = prefix_var.as_ref()
                    && let TokenKind::Identifier(_) = &token.kind
                {
                    self.field_write_prefix_positions.push(token.span.start);
                }
            }
            Var::Index(idx) => {
                if let Expression::Var(prefix_var) = &idx.prefix
                    && let Var::Name(token) = prefix_var.as_ref()
                    && let TokenKind::Identifier(_) = &token.kind
                {
                    self.field_write_prefix_positions.push(token.span.start);
                }
            }
        }
    }
}

impl Visitor for MutationCollector {
    fn visit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) if local.equal_and_exprs.is_none() => {
                for attributed in local.names.iter() {
                    self.uninit_spans
                        .push((attributed.name.span.start, attributed.name.span.end));
                }
            }
            Statement::Assignment(assign) => {
                for var in assign.targets.iter() {
                    self.record_target(var);
                }
            }
            Statement::CompoundAssignment(compound) => {
                // Luau `x.foo += 1` also mutates a field.
                self.record_target(&compound.var);
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&MutatingUninitialized, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_field_write_on_uninitialized() {
        let diags = run("local x\nx.foo = 1");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'x'"));
    }

    #[test]
    fn flags_index_write_on_uninitialized() {
        let diags = run("local x\nx[1] = 2");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'x'"));
    }

    #[test]
    fn ignores_when_initialized() {
        let diags = run("local x = {}\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_when_assigned_first() {
        let diags = run("local x\nx = {}\nx.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x\n_x.foo = 1");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
