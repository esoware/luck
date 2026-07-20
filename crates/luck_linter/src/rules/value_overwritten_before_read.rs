use luck_ast::Statement;
use luck_ast::visitor::Visitor;
use luck_semantic::scope::{ReferenceKind, SymbolKind};
use luck_token::Span;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luacheck 311: a value is written to a local, then a new value is
/// written before the first is ever read. The earlier assignment is
/// dead.
///
/// Why this is intentionally conservative: a full reaching-definitions
/// analysis would require a CFG. We only look at references that occur
/// in the same lexical scope as the symbol's declaration, walking them
/// in source order. That catches the obvious linear `x = 1; x = 2`
/// pattern while sidestepping false positives from branching:
/// `local x = 1; if c then x = 2 end; return x` keeps both writes in
/// distinct scopes, so we treat the second as untracked.
pub struct ValueOverwrittenBeforeRead;

#[derive(Clone, Copy)]
enum LastEvent {
    Init { span: Span },
    Write { span: Span },
    Read,
}

impl Rule for ValueOverwrittenBeforeRead {
    fn name(&self) -> &'static str {
        "value_overwritten_before_read"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "value assigned to a local is overwritten before it is read"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        // Determine which locals have an initializer (Init event at the
        // declaration site). The reference list does not include the
        // declaration itself, so we need an AST pass to find this.
        let mut initialized = InitializedCollector::default();
        initialized.visit_block(block);

        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.kind != SymbolKind::Local {
                continue;
            }
            if symbol.name == "_" || symbol.name.starts_with('_') {
                continue;
            }
            // A closure can read the captured value at any time between
            // the textual write positions - source order proves nothing
            // for upvalues, so stay silent.
            if symbol.is_upvalue {
                continue;
            }

            // All references, sorted by source position. References from
            // nested scopes are conservatively downgraded to reads: a
            // nested read really is a read, and a nested write is
            // conditional (branch/loop) so it proves no overwrite.
            let mut refs: Vec<(u32, ReferenceKind)> = symbol
                .reference_ids
                .iter()
                .map(|&ref_id| {
                    let r = &semantic.scope_tree.references[ref_id.index()];
                    let kind = if r.scope == symbol.scope {
                        r.kind
                    } else {
                        ReferenceKind::Read
                    };
                    (r.span.start, kind)
                })
                .collect();
            refs.sort_by_key(|(start, _)| *start);

            let has_initializer = initialized.is_initialized(symbol.definition_span);

            let mut last: Option<LastEvent> = if has_initializer {
                Some(LastEvent::Init {
                    span: symbol.definition_span,
                })
            } else {
                None
            };

            for (start, kind) in &refs {
                let ref_span = Span::new(*start, *start);
                match kind {
                    ReferenceKind::Read => {
                        last = Some(LastEvent::Read);
                    }
                    ReferenceKind::Write => match last {
                        Some(LastEvent::Init { span }) => {
                            diagnostics.push(make_diagnostic(&symbol.name, span));
                            last = Some(LastEvent::Write { span: ref_span });
                        }
                        Some(LastEvent::Write { span }) => {
                            diagnostics.push(make_diagnostic(&symbol.name, span));
                            last = Some(LastEvent::Write { span: ref_span });
                        }
                        Some(LastEvent::Read) | None => {
                            last = Some(LastEvent::Write { span: ref_span });
                        }
                    },
                    ReferenceKind::ReadWrite => {
                        // Compound assignment: reads then writes. The
                        // read clears any pending Init/Write before the
                        // write registers.
                        last = Some(LastEvent::Write { span: ref_span });
                    }
                }
            }
        }

        diagnostics
    }
}

fn make_diagnostic(name: &str, span: Span) -> LintDiagnostic {
    LintDiagnostic::new(
        "value_overwritten_before_read",
        format!("value assigned to '{name}' is overwritten before it is read"),
        span,
    )
    .with_help("remove the earlier assignment or read it first")
}

/// The scope tree records reads and writes but not whether an initializer
/// accompanies the declaration itself, so we cannot tell from the scope tree
/// alone whether a local started life with a value.
#[derive(Default)]
struct InitializedCollector {
    /// Sorted `(definition_span.start, definition_span.end)` for every
    /// local declared with an initializer.
    spans: Vec<(u32, u32)>,
}

impl InitializedCollector {
    fn is_initialized(&self, span: Span) -> bool {
        self.spans
            .iter()
            .any(|(start, end)| *start == span.start && *end == span.end)
    }
}

impl<'ast> Visitor<'ast> for InitializedCollector {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        if let Statement::LocalAssignment(local) = stmt
            && local.exprs.is_some()
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
        crate::test_support::run_rule(&ValueOverwrittenBeforeRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_init_then_overwrite() {
        let diags = run("local x = 1\nx = 2\nreturn x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'x'"));
    }

    #[test]
    fn ignores_branched_overwrite() {
        let diags = run("local x = 1\nif c then x = 2 end\nreturn x");
        assert!(
            diags.is_empty(),
            "branched writes must not fire (conservative): {diags:?}"
        );
    }

    #[test]
    fn ignores_read_between_writes() {
        let diags = run("local x = 1\nprint(x)\nx = 2\nreturn x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_single_init() {
        let diags = run("local x = 1\nreturn x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_two_post_decl_writes() {
        // Write then write, no read between: the first write is dead.
        let diags = run("local x\nx = 1\nx = 2\nreturn x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x = 1\n_x = 2\nreturn _x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
