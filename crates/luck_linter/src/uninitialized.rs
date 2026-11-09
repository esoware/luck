//! Locals declared without a value and the bare names field writes go
//! through, for the rules that follow a local before anything assigns it.

use luck_ast::{Expression, Statement, Var, is_multi_value};
use luck_semantic::{Reference, Symbol, SymbolKind};
use luck_token::Span;

/// What the scope tree doesn't record, sorted by position once [`Self::sort`]
/// has run.
#[derive(Debug, Default)]
pub(crate) struct UninitializedLocals {
    /// The names `local` statements declare without a value, as in `local x`
    /// or the `b` of `local a, b = 1`.
    name_spans: Vec<Span>,
    /// Where each bare name starts that a field or index write goes through,
    /// as in `x.foo = 1`.
    field_write_prefix_starts: Vec<u32>,
}

impl UninitializedLocals {
    pub(crate) fn record(&mut self, stmt: &Statement) {
        match stmt {
            Statement::LocalAssignment(local) => {
                // A call or `...` last can fill every name after it.
                let value_count = local.exprs.as_ref().map_or(0, |exprs| {
                    if exprs.last().is_some_and(is_multi_value) {
                        usize::MAX
                    } else {
                        exprs.items.len()
                    }
                });
                self.name_spans.extend(
                    local.names.iter().skip(value_count).map(|attributed| attributed.name.span),
                );
            }
            Statement::Assignment(assignment) => {
                for var in assignment.targets.iter() {
                    self.record_target(var);
                }
            }
            // Luau: `x.foo += 1` also mutates a field.
            Statement::CompoundAssignment(compound) => self.record_target(&compound.var),
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }

    fn record_target(&mut self, var: &Var) {
        let prefix = match var {
            Var::Name(_) => return,
            Var::FieldAccess(field_access) => &field_access.prefix,
            Var::Index(index) => &index.prefix,
        };
        if let Expression::Var(Var::Name(token)) = prefix
            && token.identifier().is_some()
        {
            self.field_write_prefix_starts.push(token.span.start);
        }
    }

    /// Restores position order, which a function inside an earlier target or
    /// annotation breaks by recording after its statement's later names.
    pub(crate) fn sort(&mut self) {
        self.name_spans.sort_unstable_by_key(|span| span.start);
        self.field_write_prefix_starts.sort_unstable();
    }

    /// Whether `symbol` is a local declared without a value of its own.
    pub(crate) fn contains(&self, symbol: &Symbol) -> bool {
        symbol.kind == SymbolKind::Local
            && self
                .name_spans
                .binary_search_by_key(&symbol.definition_span.start, |span| span.start)
                .is_ok()
    }

    /// Whether `reference` is the bare name a field or index write goes
    /// through. The scope tree records that name as a plain read.
    pub(crate) fn is_field_write_prefix(&self, reference: &Reference) -> bool {
        self.field_write_prefix_starts.binary_search(&reference.span.start).is_ok()
    }
}
