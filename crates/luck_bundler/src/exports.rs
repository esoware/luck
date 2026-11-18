use luck_ast::{Block, LocalAssignment, LocalFunction, Statement};
use luck_semantic::{ScopeKind, ScopeTree, Symbol, SymbolId};
use luck_token::{LuaVersion, Span, TokenKind, line_breaks, offset_to_index};
use rustc_hash::FxHashSet;

use crate::emitter::Edit;

/// How one module's top-level `export` declarations lower.
///
/// Luau rejects `export` below the top level, and every bundled body sits
/// inside a loader function. Type exports become private aliases. A value
/// export stays a local read by a frozen table the module returns, the form the
/// export RFC gives ("`return table.freeze({TAU = TAU})`"), unless that loses
/// its meaning. An export a closure can write after the module returns, or a
/// later local hides from the module's end, lives in the export table itself,
/// the RFC's desugared `_EXP`, so the write raises as a frozen table does.
///
/// Empty on a target without value exports.
#[derive(Debug, Default)]
pub struct Exports<'ast> {
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
    /// The plan for `block`, whose names `source` holds and `tree` resolves,
    /// when it exports a value.
    pub(crate) fn new(block: &Block<'_>, tree: &ScopeTree<'_>, source: &'ast str) -> Self {
        let exported: Vec<&Symbol> =
            tree.symbols().iter().filter(|symbol| symbol.is_exported).collect();
        let exported_symbol = |definition: Span| {
            exported.iter().copied().find(|symbol| symbol.definition_span == definition)
        };
        // A later local of the export's own scope hides it from the module's
        // end.
        let shadowed: FxHashSet<SymbolId> = tree
            .symbols()
            .iter()
            .filter_map(|symbol| {
                let hidden = tree.symbol(symbol.shadows?);
                (hidden.is_exported && hidden.scope == symbol.scope).then_some(hidden.id)
            })
            .collect();
        let needs_table = |symbol: &Symbol| {
            shadowed.contains(&symbol.id) || is_written_from_a_closure(tree, symbol)
        };
        let mut exports = Self { has_values: true, ..Self::default() };
        let mut table_symbols: Vec<&Symbol> = Vec::new();
        for stmt in &block.stmts {
            let (span, names): (Span, Vec<Span>) = match stmt {
                Statement::LocalAssignment(local) if local.kind.is_exported() => {
                    (local.span, local.names.iter().map(|binding| binding.name.span).collect())
                }
                Statement::LocalFunction(function) if function.kind.is_exported() => {
                    (function.span, vec![function.name.span])
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
                | Statement::EmptyStatement(_)
                | Statement::Goto(_)
                | Statement::Label(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_)
                | Statement::GlobalStar(_)
                | Statement::Break(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => continue,
            };
            let statement_symbols: Vec<&Symbol> =
                names.iter().filter_map(|&name| exported_symbol(name)).collect();
            if statement_symbols.iter().any(|symbol| needs_table(symbol)) {
                exports.table_statements.push(span.start);
                table_symbols.extend(statement_symbols);
            } else {
                exports.local_names.extend(
                    statement_symbols.iter().map(|symbol| symbol.definition_span.text(source)),
                );
            }
        }
        exports.field_references = table_symbols
            .iter()
            .flat_map(|symbol| tree.resolved_references(symbol.id))
            .map(|&id| tree.reference(id).span)
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
        source: &str,
        version: LuaVersion,
        prefix: &str,
        edits: &mut Vec<(Span, Edit<'_>)>,
    ) {
        let lowering = TextLowerer { source, version, prefix };
        for stmt in &block.stmts {
            let (span, text) = match stmt {
                // Luau
                Statement::TypeDeclaration(type_declaration) if type_declaration.is_exported => {
                    (export_keyword_span(type_declaration.span), String::new())
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
                | Statement::EmptyStatement(_)
                | Statement::Goto(_)
                | Statement::Label(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_)
                | Statement::GlobalStar(_)
                | Statement::Break(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
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

/// Whether `spans`, sorted by start and none starting where another does,
/// holds `span`.
pub(crate) fn contains_span(spans: &[Span], span: Span) -> bool {
    spans
        .binary_search_by_key(&span.start, |candidate| candidate.start)
        .is_ok_and(|i| spans.get(i) == Some(&span))
}

/// Whether the module exports a value, so its bundled wrapper returns the
/// export table.
pub(crate) fn has_value_exports(block: &Block<'_>) -> bool {
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
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    })
}

/// A closure can write such an export after the module returns.
fn is_written_from_a_closure(tree: &ScopeTree<'_>, symbol: &Symbol) -> bool {
    tree.resolved_references(symbol.id).iter().any(|&id| {
        let reference = tree.reference(id);
        reference.kind.is_write()
            && tree.scope(tree.scope(reference.scope).function).kind == ScopeKind::Function
    })
}

struct TextLowerer<'input> {
    source: &'input str,
    version: LuaVersion,
    prefix: &'input str,
}

impl TextLowerer<'_> {
    fn table_field(&self, name: &str) -> String {
        format!("{}exports.{name}", self.prefix)
    }

    /// `export local` and `export const`.
    fn local_edit(&self, local: &LocalAssignment<'_>, is_table: bool) -> (Span, String) {
        if !is_table {
            return (export_keyword_span(local.span), String::new());
        }
        let names_end = local.names.last().map_or(local.span, |last_name| {
            last_name.type_annotation.as_ref().map_or(last_name.name.span, luck_ast::Type::span)
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
        let keyword = self.export_keyword_of_function(function);
        if !is_table {
            return (keyword, "local".to_owned());
        }
        let signature = Span::new(keyword.start, function.name.span.end);
        let mut replacement = format!("function {}", self.table_field(function.name.text.as_str()));
        self.push_line_breaks(signature, &mut replacement);
        (signature, replacement)
    }

    /// An attributed statement's span starts at its first attribute, so the
    /// keyword is found by lexing what separates the last attribute from the
    /// name: a comment there can mention `export` too.
    fn export_keyword_of_function(&self, function: &LocalFunction<'_>) -> Span {
        let Some(attribute) = function.attributes.last() else {
            return export_keyword_span(function.span);
        };
        let gap_start = offset_to_index(attribute.span.end);
        let gap = self
            .source
            .get(gap_start..offset_to_index(function.name.span.start))
            .unwrap_or_default();
        luck_lexer::lex(gap, self.version)
            .tokens
            .iter()
            .find(|token| token.kind == TokenKind::Identifier && token.text(gap) == "export")
            .map_or_else(
                || export_keyword_span(function.span),
                |token| {
                    let start = gap_start.saturating_add(offset_to_index(token.span.start));
                    Span::from_indices(start, start.saturating_add("export".len()))
                },
            )
    }

    /// `span`'s line breaks as plain newlines, so the line map stays 1:1.
    fn push_line_breaks(&self, span: Span, text: &mut String) {
        for _ in line_breaks(span.text(self.source), self.version) {
            text.push('\n');
        }
    }
}

fn export_keyword_span(stmt_span: Span) -> Span {
    let start = offset_to_index(stmt_span.start);
    Span::from_indices(start, start.saturating_add("export".len()))
}
