//! Luau `export` declarations, lowered for bodies inside loader functions.

use luck_ast::{Block, LocalAssignment, LocalFunction, Statement};
use luck_semantic::{ScopeId, ScopeKind, ScopeTree, Symbol, SymbolId};
use luck_token::{LuaVersion, Span, line_breaks, offset_to_index};
use rustc_hash::{FxHashMap, FxHashSet};

/// The rewrites that lower one module's top-level `export` declarations.
///
/// Luau rejects `export` below the top level, and every bundled body sits
/// inside a loader function. Type exports become private aliases. A value
/// export stays a local read by a frozen table the module returns, the form the
/// export RFC gives ("`return table.freeze({TAU = TAU})`"), unless that loses
/// its meaning. An export a closure can write after the module returns, or a
/// later local hides from the module's end, lives in the export table itself,
/// the RFC's desugared `_EXP`, so the write raises as a frozen table does.
#[derive(Debug, Default)]
pub(crate) struct ExportLowering {
    pub edits: Vec<(Span, String)>,
    /// Opens the wrapper when an export lives in the table.
    pub table_declaration: Option<String>,
    /// Ends the module with its frozen export table. `None` when it exports
    /// no value.
    pub closing: Option<String>,
}

/// Whether the module exports a value, so its bundled wrapper returns the
/// export table.
pub(crate) fn has_value_exports(block: &Block) -> bool {
    block.stmts.iter().any(|stmt| match stmt {
        Statement::LocalAssignment(local) => local.is_exported,
        Statement::LocalFunction(function) => function.is_exported,
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

pub(crate) fn lower_exports(
    block: &Block,
    source: &str,
    version: LuaVersion,
    prefix: &str,
) -> ExportLowering {
    let tree = has_value_exports(block).then(|| luck_semantic::build_scope_tree(block, version));
    let mut lowering = ValueLowering::new(tree.as_ref(), source, version, prefix);
    let mut edits = Vec::new();
    for stmt in &block.stmts {
        match stmt {
            // Luau
            Statement::TypeDeclaration(type_declaration) if type_declaration.is_exported => {
                edits.push((export_keyword_span(type_declaration.span), String::new()));
            }
            Statement::LocalAssignment(local) if local.is_exported => {
                lowering.lower_local(local, &mut edits);
            }
            Statement::LocalFunction(function) if function.is_exported => {
                lowering.lower_function(function, &mut edits);
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
            | Statement::Error(_) => {}
        }
    }
    lowering.finish(edits)
}

/// Lowers the module's value exports statement by statement.
struct ValueLowering<'input> {
    tree: Option<&'input ScopeTree>,
    /// The symbol of each exported name, by its definition span.
    exported_symbols: FxHashMap<Span, &'input Symbol>,
    /// Exports a later module-level local hides, by id.
    shadowed: FxHashSet<SymbolId>,
    /// Exports living in the export table, chosen statement by statement.
    table_exports: Vec<SymbolId>,
    /// Exports staying locals, in declaration order.
    local_exports: Vec<&'input str>,
    source: &'input str,
    version: LuaVersion,
    prefix: &'input str,
}

impl<'input> ValueLowering<'input> {
    fn new(
        tree: Option<&'input ScopeTree>,
        source: &'input str,
        version: LuaVersion,
        prefix: &'input str,
    ) -> Self {
        let exported_symbols = tree
            .into_iter()
            .flat_map(|scope_tree| &scope_tree.symbols)
            .filter(|symbol| symbol.is_exported)
            .map(|symbol| (symbol.definition_span, symbol))
            .collect();
        Self {
            tree,
            exported_symbols,
            shadowed: tree.map(module_level_shadows).unwrap_or_default(),
            table_exports: Vec::new(),
            local_exports: Vec::new(),
            source,
            version,
            prefix,
        }
    }

    /// Whether `symbol` can't stay a plain local read at the module's end.
    fn needs_table(&self, symbol: &Symbol) -> bool {
        self.shadowed.contains(&symbol.id)
            || self.tree.is_some_and(|tree| is_written_from_a_closure(tree, symbol))
    }

    fn table_field(&self, name: &str) -> String {
        format!("{}exports.{name}", self.prefix)
    }

    /// `export local` and `export const`. A declaration can't mix locals and
    /// fields, so one table export moves all its names to the table.
    fn lower_local(&mut self, local: &'input LocalAssignment, edits: &mut Vec<(Span, String)>) {
        let symbols: Vec<&'input Symbol> = local
            .names
            .iter()
            .filter_map(|binding| self.exported_symbols.get(&binding.name.span).copied())
            .collect();
        if !symbols.iter().any(|symbol| self.needs_table(symbol)) {
            self.local_exports
                .extend(local.names.iter().filter_map(|binding| binding.name.identifier()));
            edits.push((export_keyword_span(local.span), String::new()));
            return;
        }
        self.table_exports.extend(symbols.iter().map(|symbol| symbol.id));

        let Some(last_name) = local.names.last() else {
            return;
        };
        let names_end =
            last_name.type_annotation.as_ref().map_or(last_name.name.span, luck_ast::Type::span);
        let declaration = Span::new(local.span.start, names_end.end);
        let targets = symbols
            .iter()
            .map(|symbol| self.table_field(symbol.name.as_str()))
            .collect::<Vec<_>>()
            .join(",");
        let initializer = if local.exprs.is_none() { " = nil" } else { "" };
        let newlines = self.newlines_in(declaration);
        edits.push((declaration, format!("{targets}{initializer}{newlines}")));
    }

    /// `export function`, whose name always binds a local, or a field when the
    /// export lives in the table.
    fn lower_function(&mut self, function: &'input LocalFunction, edits: &mut Vec<(Span, String)>) {
        let keyword = self.export_keyword_of_function(function);
        let Some(symbol) = self.exported_symbols.get(&function.name.span).copied() else {
            return;
        };
        if !self.needs_table(symbol) {
            self.local_exports.extend(function.name.identifier());
            edits.push((keyword, "local".to_owned()));
            return;
        }
        self.table_exports.push(symbol.id);
        let signature = Span::new(keyword.start, function.name.span.end);
        let newlines = self.newlines_in(signature);
        let field = self.table_field(symbol.name.as_str());
        edits.push((signature, format!("function {field}{newlines}")));
    }

    /// An attributed statement's span starts at its first attribute, so the
    /// keyword is found by lexing what separates the last attribute from the
    /// name: a comment there can mention `export` too.
    fn export_keyword_of_function(&self, function: &LocalFunction) -> Span {
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
            .find(|token| token.identifier() == Some("export"))
            .map_or_else(
                || export_keyword_span(function.span),
                |token| {
                    let start = gap_start.saturating_add(offset_to_index(token.span.start));
                    Span::from_indices(start, start.saturating_add("export".len()))
                },
            )
    }

    /// `span`'s line breaks as plain newlines, so the line map stays 1:1.
    fn newlines_in(&self, span: Span) -> String {
        "\n".repeat(line_breaks(span.text(self.source), self.version).count())
    }

    fn finish(self, mut edits: Vec<(Span, String)>) -> ExportLowering {
        let Some(tree) = self.tree else {
            return ExportLowering { edits, table_declaration: None, closing: None };
        };
        for &id in &self.table_exports {
            let symbol = tree.symbol(id);
            let field = self.table_field(symbol.name.as_str());
            edits.extend(
                symbol
                    .reference_ids
                    .iter()
                    .map(|reference_id| (tree.reference(*reference_id).span, field.clone())),
            );
        }

        let prefix = self.prefix;
        let (table_declaration, closing) = if self.table_exports.is_empty() {
            let fields =
                self.local_exports.iter().map(|name| format!("{name}={name}")).collect::<Vec<_>>();
            (None, format!("return {prefix}freeze({{{}}})\n", fields.join(",")))
        } else {
            let copies = self
                .local_exports
                .iter()
                .map(|name| format!("{prefix}exports.{name}={name}\n"))
                .collect::<Vec<_>>()
                .concat();
            (
                Some(format!("local {prefix}exports={{}}")),
                format!("{copies}return {prefix}freeze({prefix}exports)\n"),
            )
        };
        ExportLowering { edits, table_declaration, closing: Some(closing) }
    }
}

/// Exports a later local at their own (module) scope hides, by id.
fn module_level_shadows(tree: &ScopeTree) -> FxHashSet<SymbolId> {
    tree.symbols
        .iter()
        .filter(|symbol| {
            symbol.shadows.is_some_and(|hidden| {
                tree.symbol(hidden).is_exported && tree.symbol(hidden).scope == symbol.scope
            })
        })
        .filter_map(|symbol| symbol.shadows)
        .collect()
}

fn is_written_from_a_closure(tree: &ScopeTree, symbol: &Symbol) -> bool {
    symbol.reference_ids.iter().any(|id| {
        let reference = tree.reference(*id);
        reference.kind.is_write() && is_inside_a_function(tree, reference.scope)
    })
}

fn is_inside_a_function(tree: &ScopeTree, start: ScopeId) -> bool {
    let mut current = Some(start);
    while let Some(id) = current {
        let scope = tree.scope(id);
        if scope.kind == ScopeKind::Function {
            return true;
        }
        current = scope.parent;
    }
    false
}

/// The `export` keyword opening a statement that starts at `stmt_span`.
fn export_keyword_span(stmt_span: Span) -> Span {
    let start = offset_to_index(stmt_span.start);
    Span::from_indices(start, start.saturating_add("export".len()))
}
