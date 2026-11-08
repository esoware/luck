//! Locals declared without a value, for the rules that follow what a local
//! holds before anything assigns it.

use luck_ast::{Block, Statement, Visitor};
use luck_semantic::{Reference, ScopeTree, Symbol, SymbolKind};
use luck_token::Span;

/// The name spans of the locals that `local` statements declare without a
/// value, as in `local x`. The scope tree doesn't record initializers.
#[derive(Default)]
pub(crate) struct UninitializedLocals {
    name_spans: Vec<Span>,
}

impl UninitializedLocals {
    pub(crate) fn collect(block: &Block) -> Self {
        let mut locals = Self::default();
        locals.visit_block(block);
        locals
    }

    /// Records the names `stmt` declares, if it's a `local` statement with
    /// no values.
    pub(crate) fn record(&mut self, stmt: &Statement) {
        if let Statement::LocalAssignment(local) = stmt
            && local.exprs.is_none()
        {
            self.name_spans.extend(local.names.iter().map(|attributed| attributed.name.span));
        }
    }

    /// Whether `symbol` is a local declared without a value.
    pub(crate) fn contains(&self, symbol: &Symbol) -> bool {
        symbol.kind == SymbolKind::Local && self.name_spans.contains(&symbol.definition_span)
    }
}

impl<'ast> Visitor<'ast> for UninitializedLocals {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        self.record(stmt);
        self.walk_statement(stmt);
    }
}

/// The first reference to `symbol` in source order, from any scope.
///
/// Searching only the declaring scope would miss a write in an `if` or loop
/// body, as in `local x if c then x = 1 end print(x)`.
pub(crate) fn first_reference<'tree>(
    symbol: &Symbol,
    tree: &'tree ScopeTree,
) -> Option<&'tree Reference> {
    symbol
        .reference_ids
        .iter()
        .map(|&reference_id| tree.reference(reference_id))
        .min_by_key(|reference| reference.span.start)
}
