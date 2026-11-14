use luck_ast::{Block, LocalAssignment, LocalFunction, Statement};
use luck_semantic::{ScopeId, ScopeKind, ScopeTree, Symbol, SymbolId};
use luck_token::{CompactString, LuaVersion, Span, line_breaks, offset_to_index};
use rustc_hash::{FxHashMap, FxHashSet};

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
pub(crate) struct ExportPlan {
    /// Exports that stay locals, in declaration order.
    pub local_exports: Vec<CompactString>,
    /// The span starts of the statements whose exports live in the table. A
    /// declaration can't mix locals and fields, so one table export moves all
    /// its names there.
    pub table_statements: FxHashSet<u32>,
    /// The span of each read and write of an export that lives in the table,
    /// which becomes a field of the table.
    pub field_references: FxHashSet<Span>,
    /// Whether the module exports a value, so its wrapper returns the export
    /// table.
    pub has_values: bool,
}

impl ExportPlan {
    /// The plan for `block`, which exports nothing on a target without value
    /// exports.
    pub(crate) fn new(block: &Block, version: LuaVersion) -> Self {
        if !version.has_value_exports() || !has_value_exports(block) {
            return Self::default();
        }
        let tree = luck_semantic::build_scope_tree(block, version);
        let exported_symbols: FxHashMap<Span, &Symbol> = tree
            .symbols
            .iter()
            .filter(|symbol| symbol.is_exported)
            .map(|symbol| (symbol.definition_span, symbol))
            .collect();
        let shadowed = module_level_shadows(&tree);
        let needs_table = |symbol: &Symbol| {
            shadowed.contains(&symbol.id) || is_written_from_a_closure(&tree, symbol)
        };
        let mut plan = Self { has_values: true, ..Self::default() };
        let mut table_exports: Vec<&Symbol> = Vec::new();
        for stmt in &block.stmts {
            let (span, symbols): (Span, Vec<&Symbol>) = match stmt {
                Statement::LocalAssignment(local) if local.is_exported => (
                    local.span,
                    local
                        .names
                        .iter()
                        .filter_map(|binding| exported_symbols.get(&binding.name.span).copied())
                        .collect(),
                ),
                Statement::LocalFunction(function) if function.kind.is_exported() => (
                    function.span,
                    exported_symbols.get(&function.name.span).copied().into_iter().collect(),
                ),
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
            if symbols.iter().any(|symbol| needs_table(symbol)) {
                plan.table_statements.insert(span.start);
                table_exports.extend(symbols);
            } else {
                plan.local_exports.extend(symbols.iter().map(|symbol| symbol.name.clone()));
            }
        }
        plan.field_references = table_exports
            .iter()
            .flat_map(|symbol| &symbol.reference_ids)
            .map(|id| tree.reference(*id).span)
            .collect();
        plan
    }

    pub(crate) fn has_table(&self) -> bool {
        !self.table_statements.is_empty()
    }

    /// Whether the exported statement at `span` moves its exports to the
    /// table.
    pub(crate) fn is_table_statement(&self, span: Span) -> bool {
        self.table_statements.contains(&span.start)
    }
}

/// Whether the module exports a value, so its bundled wrapper returns the
/// export table.
pub(crate) fn has_value_exports(block: &Block) -> bool {
    block.stmts.iter().any(|stmt| match stmt {
        Statement::LocalAssignment(local) => local.is_exported,
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

/// The splices that lower a module's exports in its source text, and the
/// lines around its body.
#[derive(Debug, Default)]
pub(crate) struct TextLowering {
    pub edits: Vec<(Span, String)>,
    /// Opens the wrapper when an export lives in the table.
    pub table_declaration: Option<String>,
    /// Ends the module with its frozen export table. `None` when it exports
    /// no value.
    pub closing: Option<String>,
}

pub(crate) fn lower_exports_in_text(
    plan: &ExportPlan,
    block: &Block,
    source: &str,
    version: LuaVersion,
    prefix: &str,
) -> TextLowering {
    let lowering = TextLowerer { source, version, prefix };
    let mut edits = Vec::new();
    for stmt in &block.stmts {
        match stmt {
            // Luau
            Statement::TypeDeclaration(type_declaration) if type_declaration.is_exported => {
                edits.push((export_keyword_span(type_declaration.span), String::new()));
            }
            Statement::LocalAssignment(local) if local.is_exported => {
                edits.push(lowering.local_edit(local, plan.is_table_statement(local.span)));
            }
            Statement::LocalFunction(function) if function.kind.is_exported() => {
                edits
                    .push(lowering.function_edit(function, plan.is_table_statement(function.span)));
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
    edits.extend(
        plan.field_references.iter().map(|span| (*span, lowering.table_field(span.text(source)))),
    );
    if !plan.has_values {
        return TextLowering { edits, table_declaration: None, closing: None };
    }
    let (table_declaration, closing) = if plan.has_table() {
        let copies = plan
            .local_exports
            .iter()
            .map(|name| format!("{prefix}exports.{name}={name}\n"))
            .collect::<Vec<_>>()
            .concat();
        (
            Some(format!("local {prefix}exports={{}}")),
            format!("{copies}return {prefix}freeze({prefix}exports)\n"),
        )
    } else {
        let fields =
            plan.local_exports.iter().map(|name| format!("{name}={name}")).collect::<Vec<_>>();
        (None, format!("return {prefix}freeze({{{}}})\n", fields.join(",")))
    };
    TextLowering { edits, table_declaration, closing: Some(closing) }
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
    fn local_edit(&self, local: &LocalAssignment, is_table: bool) -> (Span, String) {
        let names_end = local.names.last().map_or(local.span, |last_name| {
            last_name.type_annotation.as_ref().map_or(last_name.name.span, luck_ast::Type::span)
        });
        if !is_table {
            return (export_keyword_span(local.span), String::new());
        }
        let declaration = Span::new(local.span.start, names_end.end);
        let targets = local
            .names
            .iter()
            .filter_map(|binding| binding.name.identifier())
            .map(|name| self.table_field(name))
            .collect::<Vec<_>>()
            .join(",");
        let initializer = if local.exprs.is_none() { " = nil" } else { "" };
        let newlines = self.newlines_in(declaration);
        (declaration, format!("{targets}{initializer}{newlines}"))
    }

    /// `export function`, whose name always binds a local, or a field when the
    /// export lives in the table.
    fn function_edit(&self, function: &LocalFunction, is_table: bool) -> (Span, String) {
        let keyword = self.export_keyword_of_function(function);
        if !is_table {
            return (keyword, "local".to_owned());
        }
        let signature = Span::new(keyword.start, function.name.span.end);
        let newlines = self.newlines_in(signature);
        let field = self.table_field(function.name.identifier().unwrap_or_default());
        (signature, format!("function {field}{newlines}"))
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
}

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

fn export_keyword_span(stmt_span: Span) -> Span {
    let start = offset_to_index(stmt_span.start);
    Span::from_indices(start, start.saturating_add("export".len()))
}
