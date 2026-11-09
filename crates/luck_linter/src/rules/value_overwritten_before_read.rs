use luck_semantic::{ReferenceKind, SymbolKind};
use luck_token::Span;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags a value written to a local and then overwritten before anything
/// reads it (Luacheck 311).
///
/// This isn't a full reaching-definitions analysis. It tracks writes only in
/// the declaring scope, in source order, and counts any reference from a
/// nested scope as a read. That catches the linear `x = 1; x = 2` and stays
/// quiet on `local x = 1; if c then x = 2 end; return x`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ValueOverwrittenBeforeRead;

impl_rule!(
    ValueOverwrittenBeforeRead,
    "value_overwritten_before_read",
    Correctness,
    Warning,
    "value assigned to a local is overwritten before it is read"
);

impl WholeRule for ValueOverwrittenBeforeRead {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let scope_tree = &context.semantic.scope_tree;
        let uninitialized_locals = context.uninitialized_locals;
        let mut findings = Vec::new();

        for symbol in &scope_tree.symbols {
            if symbol.kind != SymbolKind::Local || symbol.name.starts_with('_') {
                continue;
            }
            // A closure can read a captured local at any time, so source
            // order proves nothing for upvalues.
            if symbol.is_upvalue {
                continue;
            }

            // A reference from a nested scope counts as a read, since a
            // write inside a branch or loop might not run.
            let mut references: Vec<(u32, ReferenceKind)> = symbol
                .reference_ids
                .iter()
                .map(|&reference_id| {
                    let reference = scope_tree.reference(reference_id);
                    let kind = if reference.scope == symbol.scope {
                        reference.kind
                    } else {
                        ReferenceKind::Read
                    };
                    (reference.span.start, kind)
                })
                .collect();
            references.sort_by_key(|&(start, _)| start);

            // Where the value no reference has read yet was written, if any.
            // A `Local` comes only from a `local` statement, so one not
            // declared bare starts with its initializer unread.
            let mut unread_write =
                (!uninitialized_locals.contains(symbol)).then_some(symbol.definition_span);
            for (start, kind) in references {
                match kind {
                    ReferenceKind::Read => unread_write = None,
                    ReferenceKind::Write => {
                        if let Some(span) = unread_write {
                            findings.push(overwritten_finding(&symbol.name, span));
                        }
                        unread_write = Some(Span::new(start, start));
                    }
                    // A compound assignment reads then writes, so the read
                    // clears any pending write before its own registers.
                    ReferenceKind::ReadWrite => unread_write = Some(Span::new(start, start)),
                }
            }
        }

        findings
    }
}

fn overwritten_finding(name: &str, span: Span) -> Finding {
    Finding::new(format!("value assigned to `{name}` is overwritten before it is read"), span)
        .with_help("remove the earlier assignment or read it first")
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&ValueOverwrittenBeforeRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_initializer_then_overwrite() {
        let diags = run("local x = 1\nx = 2\nreturn x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_branched_overwrite() {
        let diags = run("local x = 1\nif c then x = 2 end\nreturn x");
        assert!(diags.is_empty(), "branched writes must not fire (conservative): {diags:?}");
    }

    #[test]
    fn ignores_read_between_writes() {
        let diags = run("local x = 1\nprint(x)\nx = 2\nreturn x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_single_initializer() {
        let diags = run("local x = 1\nreturn x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_two_writes_after_declaration() {
        // Write then write with no read between, so the first is dead.
        let diags = run("local x\nx = 1\nx = 2\nreturn x");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_prefixed() {
        let diags = run("local _x = 1\n_x = 2\nreturn _x");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn bug_ignores_assignment_reading_its_own_target() {
        let diags = run("local x = 1\nx = x + 1\nreturn x");
        assert!(diags.is_empty(), "the right-hand side reads the old value: {diags:?}");
    }

    #[test]
    fn bug_ignores_write_skipped_by_goto() {
        let diags = run("local x = 1\nif c then goto skip end\nx = 2\n::skip::\nprint(x)");
        assert!(diags.is_empty(), "the `goto` path reads the initializer: {diags:?}");
    }

    #[test]
    fn bug_points_at_the_overwritten_assignment() {
        let source = "local x\nx = 1\nx = 2\nreturn x";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].span.text(source), "x", "the span should cover `x`: {diags:?}");
    }
}
