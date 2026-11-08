//! Luau `export` declarations, lowered for bodies inside loader functions.

use luck_ast::{Block, Statement};
use luck_token::{Span, offset_to_index};

/// The rewrites that lower module-level `export` declarations, and the
/// exported value names.
///
/// Luau rejects `export` below the top level, and every bundled body sits
/// inside a loader function. Type exports become private aliases, and value
/// exports become plain declarations plus the frozen table Luau would
/// implicitly return for the module.
pub(crate) fn lower_exports(block: &Block) -> (Vec<(Span, String)>, Vec<&str>) {
    let mut edits = Vec::new();
    let mut value_exports = Vec::new();
    for stmt in &block.stmts {
        match stmt {
            // Luau
            Statement::TypeDeclaration(type_declaration) if type_declaration.is_exported => {
                edits.push((export_keyword_span(type_declaration.span), String::new()));
            }
            Statement::LocalAssignment(local) if local.is_exported => {
                value_exports
                    .extend(local.names.iter().filter_map(|binding| binding.name.identifier()));
                edits.push((export_keyword_span(local.span), String::new()));
            }
            Statement::LocalFunction(function) if function.is_exported => {
                value_exports.extend(function.name.identifier());
                // `export function` declares a local function. An
                // attributed statement's span starts at its first
                // attribute, so the rewrite covers everything from the
                // last attribute to the name, comments included.
                if let Some(attribute) = function.attributes.last() {
                    edits.push((
                        Span::new(attribute.span.end, function.name.span.start),
                        "\nlocal function ".to_owned(),
                    ));
                } else {
                    edits.push((export_keyword_span(function.span), "local".to_owned()));
                }
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
    (edits, value_exports)
}

/// The `export` keyword opening a statement that starts at `stmt_span`.
fn export_keyword_span(stmt_span: Span) -> Span {
    let start = offset_to_index(stmt_span.start);
    Span::from_indices(start, start.saturating_add("export".len()))
}
