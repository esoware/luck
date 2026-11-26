use luck_ast::{Block, GetSpan as _, LocalAssignment, LocalFunction, Statement};
use luck_semantic::{ScopeKind, Scoping, SymbolId};
use luck_token::{Span, line_breaks};
use rustc_hash::FxHashSet;

use crate::module::contains_span;
use crate::text_emitter::Edit;

/// How one module's top-level `export` declarations lower.
///
/// Luau rejects `export` below the top level, and every bundled body sits
/// inside a loader function. Type exports become private aliases. A value
/// export stays a local read by a frozen table the module returns, the form the
/// export RFC gives ("`return table.freeze({TAU = TAU})`"), unless that loses
/// its meaning. An export a closure can write after the module returns, or a
/// later local hides from the module's end, lives in the export table itself,
/// the RFC's desugared `_EXP`, so the write raises as a frozen table does.
#[derive(Debug, Default)]
pub(crate) struct Exports<'ast> {
    /// Exports that stay locals, in declaration order.
    pub local_names: Vec<&'ast str>,
    /// Where each statement whose exports live in the table starts, in
    /// source order. A declaration can't mix locals and fields, so one table
    /// export moves all its names there.
    pub table_statements: Vec<u32>,
    /// Each read and write of an export that lives in the table, which
    /// becomes a field of the table, by where it starts.
    pub field_references: Vec<Span>,
    /// Whether the module exports a value, so its wrapper returns the export
    /// table.
    pub has_values: bool,
}

impl<'ast> Exports<'ast> {
    /// The plan for `block`, a chunk's, whose names `source` holds and
    /// `scoping` resolves, which is empty unless it exports a value.
    pub(crate) fn new(block: &Block<'_>, scoping: &Scoping, source: &'ast [u8]) -> Self {
        if !has_value_exports(block) {
            return Self::default();
        }
        // A later local of the export's own scope hides it from the module's
        // end.
        let shadowed: FxHashSet<SymbolId> = scoping
            .symbols()
            .iter()
            .filter_map(|symbol| {
                let hidden = scoping.symbol(symbol.shadows?);
                (hidden.is_exported && hidden.scope == symbol.scope).then_some(hidden.id)
            })
            .collect();
        let needs_table = |symbol: SymbolId| {
            shadowed.contains(&symbol) || is_written_from_a_closure(scoping, symbol)
        };
        let mut exports = Self { has_values: true, ..Self::default() };
        let mut table_symbols: Vec<SymbolId> = Vec::new();
        for stmt in &block.stmts {
            let (span, statement_symbols): (Span, Vec<SymbolId>) = match stmt {
                Statement::LocalAssignment(local) if local.kind.is_exported() => (
                    local.span,
                    local
                        .names
                        .iter()
                        .filter_map(|binding| scoping.symbol_of(&binding.name))
                        .collect(),
                ),
                Statement::LocalFunction(function) if function.kind.is_exported() => {
                    (function.span, scoping.symbol_of(&function.name).into_iter().collect())
                }
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::DoBlock(_)
                | Statement::WhileLoop(_)
                | Statement::RepeatLoop(_)
                | Statement::IfStatement(_)
                | Statement::NumericFor(_)
                | Statement::GenericFor(_)
                | Statement::FunctionDecl(_)
                | Statement::LocalFunction(_)
                | Statement::LocalAssignment(_)
                | Statement::Return(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Declaration(_)
                | Statement::Error(_) => continue,
            };
            if statement_symbols.iter().any(|&symbol| needs_table(symbol)) {
                exports.table_statements.push(span.start);
                table_symbols.extend(statement_symbols);
            } else {
                exports.local_names.extend(statement_symbols.into_iter().map(|symbol| {
                    let name = scoping.symbol(symbol).definition_span.text(source);
                    str::from_utf8(name).expect("the lexer reads only ASCII names")
                }));
            }
        }
        exports.field_references = table_symbols
            .into_iter()
            .flat_map(|symbol| scoping.resolved_references(symbol))
            .map(|&id| scoping.reference(id).span)
            .collect();
        exports.field_references.sort_unstable_by_key(|span| span.start);
        exports
    }

    pub(crate) const fn has_table(&self) -> bool {
        !self.table_statements.is_empty()
    }

    /// Whether the exported statement at `span` moves its exports to the
    /// table.
    pub(crate) fn is_table_statement(&self, span: Span) -> bool {
        self.table_statements.binary_search(&span.start).is_ok()
    }

    pub(crate) fn is_field_reference(&self, span: Span) -> bool {
        contains_span(&self.field_references, span)
    }

    /// What opens the wrapper's body when an export lives in the table.
    pub(crate) fn write_table_declaration(&self, prefix: &str, output: &mut String) {
        if self.has_table() {
            output.extend(["local ", prefix, "exports={}"]);
        }
    }

    /// The lines that end the module with its frozen export table, when it
    /// exports a value.
    pub(crate) fn write_closing(&self, prefix: &str, output: &mut String) {
        if !self.has_values {
            return;
        }
        if self.has_table() {
            for name in &self.local_names {
                output.extend([prefix, "exports.", name, "=", name, "\n"]);
            }
            output.extend(["return ", prefix, "freeze(", prefix, "exports)\n"]);
            return;
        }
        output.extend(["return ", prefix, "freeze({"]);
        for (i, name) in self.local_names.iter().enumerate() {
            if i > 0 {
                output.push(',');
            }
            output.extend([*name, "=", name]);
        }
        output.push_str("})\n");
    }

    /// The splices that lower the exports in `source`, the text of `block`.
    pub(crate) fn text_edits(
        &self,
        block: &Block<'_>,
        source: &[u8],
        prefix: &str,
        edits: &mut Vec<(Span, Edit)>,
    ) {
        let lowering = TextLowerer { source, prefix };
        for stmt in &block.stmts {
            let (span, text) = match stmt {
                Statement::TypeDeclaration(type_declaration) if type_declaration.is_exported => {
                    (export_keyword_at(type_declaration.span.start), String::new())
                }
                Statement::LocalAssignment(local) if local.kind.is_exported() => {
                    lowering.local_edit(local, self.is_table_statement(local.span))
                }
                Statement::LocalFunction(function) if function.kind.is_exported() => {
                    lowering.function_edit(function, self.is_table_statement(function.span))
                }
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::DoBlock(_)
                | Statement::WhileLoop(_)
                | Statement::RepeatLoop(_)
                | Statement::IfStatement(_)
                | Statement::NumericFor(_)
                | Statement::GenericFor(_)
                | Statement::FunctionDecl(_)
                | Statement::LocalFunction(_)
                | Statement::LocalAssignment(_)
                | Statement::Return(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Declaration(_)
                | Statement::Error(_) => continue,
            };
            edits.push((span, Edit::Text(text)));
        }
        edits.extend(
            self.field_references
                .iter()
                .map(|span| (*span, Edit::Text(lowering.table_field(span.text(source))))),
        );
    }
}

/// Whether the module exports a value, so its bundled wrapper returns the
/// export table.
fn has_value_exports(block: &Block<'_>) -> bool {
    block.stmts.iter().any(|stmt| match stmt {
        Statement::LocalAssignment(local) => local.kind.is_exported(),
        Statement::LocalFunction(function) => function.kind.is_exported(),
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::Return(_)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Declaration(_)
        | Statement::Error(_) => false,
    })
}

/// A closure can write such an export after the module returns.
fn is_written_from_a_closure(scoping: &Scoping, symbol: SymbolId) -> bool {
    scoping.resolved_references(symbol).iter().any(|&id| {
        let reference = scoping.reference(id);
        reference.kind.is_write()
            && scoping.scope(scoping.scope(reference.scope).function).kind == ScopeKind::Function
    })
}

struct TextLowerer<'input> {
    source: &'input [u8],
    prefix: &'input str,
}

impl TextLowerer<'_> {
    /// `name` is a name, which is ASCII.
    fn table_field(&self, name: &[u8]) -> String {
        format!("{}exports.{}", self.prefix, name.escape_ascii())
    }

    /// `export local` and `export const`.
    fn local_edit(&self, local: &LocalAssignment<'_>, is_table: bool) -> (Span, String) {
        if !is_table {
            return (export_keyword_at(local.span.start), String::new());
        }
        let names_end = local.names.last().map_or(local.span, |last_name| {
            last_name.type_annotation.as_deref().map_or(last_name.name.span, luck_ast::Type::span)
        });
        let declaration = Span::new(local.span.start, names_end.end);
        let mut targets = String::new();
        for (i, binding) in local.names.iter().enumerate() {
            if i > 0 {
                targets.push(',');
            }
            targets.extend([self.prefix, "exports.", binding.name.text.as_str()]);
        }
        if local.exprs.is_empty() {
            targets.push_str(" = nil");
        }
        self.push_line_breaks(declaration, &mut targets);
        (declaration, targets)
    }

    /// `export function`, whose name always binds a local, or a field when the
    /// export lives in the table.
    fn function_edit(&self, function: &LocalFunction<'_>, is_table: bool) -> (Span, String) {
        if !is_table {
            return (export_keyword_at(function.keyword_start), "local".to_owned());
        }
        let signature = Span::new(function.keyword_start, function.name.span.end);
        let name = function.name.text.as_bytes();
        let mut replacement = format!("function {}", self.table_field(name));
        self.push_line_breaks(signature, &mut replacement);
        (signature, replacement)
    }

    /// `span`'s line breaks as plain newlines, so the line map stays 1:1.
    fn push_line_breaks(&self, span: Span, text: &mut String) {
        for _ in line_breaks(span.text(self.source)) {
            text.push('\n');
        }
    }
}

fn export_keyword_at(start: u32) -> Span {
    let index = start as usize;
    Span::from_indices(index, index + "export".len())
}
