//! Rename.
//!
//! Locals only: [`prepare`] refuses globals and stdlib names, and a rename is
//! refused whenever the new name could change meaning: an invalid
//! identifier, a reserved word, a known global, or a name already used
//! anywhere in the document. Refusing is safer than silently capturing a
//! shadowed name.

use luck_token::{Span, TokenKind};
use tower_lsp::jsonrpc::{Error as JsonRpcError, Result as JsonRpcResult};
use tower_lsp::lsp_types::{
    Position, PrepareRenameResponse, RenameParams, TextEdit, WorkspaceEdit,
};

use crate::document::DocumentState;
use crate::providers::cursor::local_at;

pub(crate) fn prepare(
    document: &DocumentState,
    position: Position,
) -> Option<PrepareRenameResponse> {
    let (_, span) = local_at(&document.semantic.scope_tree, document.offset(position))?;
    Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: document.range(span),
        placeholder: span.text(document.text()).to_owned(),
    })
}

/// Compute the workspace edit for a rename.
///
/// # Errors
///
/// Returns an invalid-params error carrying the refusal to show the user when
/// `params.new_name` could change what the document means.
pub(crate) fn rename(
    document: &DocumentState,
    params: &RenameParams,
) -> JsonRpcResult<Option<WorkspaceEdit>> {
    let text_document_position = &params.text_document_position;
    let tree = &document.semantic.scope_tree;
    let Some((symbol_id, _)) = local_at(tree, document.offset(text_document_position.position))
    else {
        return Ok(None);
    };
    let new_name = params.new_name.as_str();
    validate_new_name(document, new_name).map_err(JsonRpcError::invalid_params)?;

    let symbol = tree.symbol(symbol_id);
    let mut spans: Vec<Span> = symbol
        .reference_ids
        .iter()
        .map(|&reference_id| tree.reference(reference_id).span)
        .collect();
    spans.push(symbol.definition_span);
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    let edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|span| TextEdit { range: document.range(span), new_text: new_name.to_owned() })
        .collect();
    let changes = [(text_document_position.text_document.uri.clone(), edits)].into();
    Ok(Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }))
}

/// Checks that `new_name` is safe to rename a local to.
///
/// # Errors
///
/// Returns the refusal to show the user when it isn't an identifier, shadows
/// a built-in global, or is already used in the document.
fn validate_new_name(document: &DocumentState, new_name: &str) -> Result<(), String> {
    // Lexing the candidate under the document's Lua version rejects
    // keywords version-accurately (`goto` is an identifier in 5.1).
    let lexed = luck_lexer::lex(new_name, document.target.lua_version());
    let tokens: Vec<_> = lexed.tokens.iter().filter(|token| token.kind != TokenKind::Eof).collect();
    let is_identifier = lexed.errors.is_empty()
        && matches!(
            tokens.as_slice(),
            [token] if matches!(token.kind, TokenKind::Identifier(_))
                && token.span == Span::from_indices(0, new_name.len())
        );
    if !is_identifier {
        return Err(format!("`{new_name}` isn't a valid identifier"));
    }
    if document.semantic.is_known_global(new_name) {
        return Err(format!("`{new_name}` would shadow a built-in global"));
    }
    let tree = &document.semantic.scope_tree;
    let is_in_use = tree.symbols.iter().any(|symbol| symbol.name == new_name)
        || tree.references.iter().any(|reference| reference.name == new_name);
    if is_in_use {
        return Err(format!(
            "`{new_name}` is already used in this file, so renaming could capture it"
        ));
    }
    Ok(())
}
