//! Rename. Local symbols only: globals and stdlib names are refused in
//! `prepare_rename`, and the new name is rejected whenever the rename
//! could change meaning - invalid identifier, reserved word, a name
//! already used anywhere in the document, or a known global. Rejecting
//! ambiguity beats silently capturing a shadowed name.

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    PrepareRenameResponse, RenameParams, TextDocumentPositionParams, TextEdit, Url, WorkspaceEdit,
};

use crate::backend::DocumentState;
use crate::providers::cursor::{name_span_at, symbol_at};

#[must_use]
pub fn prepare_rename(
    doc: &DocumentState,
    params: &TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
    let offset = doc.line_index.offset(&doc.text, params.position);
    let tree = &doc.semantic.scope_tree;
    symbol_at(tree, offset)?;
    let span = name_span_at(tree, offset)?;
    Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: doc.line_index.range(&doc.text, span.start, span.end),
        placeholder: doc.text[span.start as usize..span.end as usize].to_string(),
    })
}

/// Compute the workspace edit for a rename, or a human-readable refusal.
pub fn rename(
    doc: &DocumentState,
    uri: &Url,
    params: &RenameParams,
) -> Result<Option<WorkspaceEdit>, String> {
    let offset = doc
        .line_index
        .offset(&doc.text, params.text_document_position.position);
    let tree = &doc.semantic.scope_tree;
    let Some(symbol_id) = symbol_at(tree, offset) else {
        return Ok(None);
    };
    let new_name = params.new_name.as_str();
    validate_new_name(doc, new_name)?;

    let symbol = &tree.symbols[symbol_id.index()];
    let mut spans: Vec<luck_token::Span> = symbol
        .reference_ids
        .iter()
        .map(|&reference_id| tree.references[reference_id.index()].span)
        .collect();
    spans.push(symbol.definition_span);
    spans.sort_by_key(|span| span.start);
    spans.dedup();

    let edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|span| TextEdit {
            range: doc.line_index.range(&doc.text, span.start, span.end),
            new_text: new_name.to_string(),
        })
        .collect();
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }))
}

fn validate_new_name(doc: &DocumentState, new_name: &str) -> Result<(), String> {
    // Lexing the candidate under the document's Lua version rejects
    // keywords version-accurately (`goto` is an identifier in 5.1).
    let lexed = luck_lexer::lex(new_name, doc.target.lua_version());
    let tokens: Vec<_> = lexed
        .tokens
        .iter()
        .filter(|token| token.kind != luck_token::TokenKind::Eof)
        .collect();
    let is_identifier = lexed.errors.is_empty()
        && tokens.len() == 1
        && matches!(tokens[0].kind, luck_token::TokenKind::Identifier(_))
        && tokens[0].span.start == 0
        && tokens[0].span.end == new_name.len() as u32;
    if !is_identifier {
        return Err(format!("'{new_name}' is not a valid identifier"));
    }
    if doc.semantic.is_known_global(new_name) {
        return Err(format!("'{new_name}' would shadow a built-in global"));
    }
    let tree = &doc.semantic.scope_tree;
    let in_use = tree.symbols.iter().any(|symbol| symbol.name == new_name)
        || tree
            .references
            .iter()
            .any(|reference| reference.name == new_name);
    if in_use {
        return Err(format!(
            "'{new_name}' is already used in this file; renaming could capture it"
        ));
    }
    Ok(())
}
