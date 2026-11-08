//! Flattens each open document's outline and filters it by a
//! case-insensitive substring query. There's no on-disk index, so only open
//! documents are searched.

#![expect(
    deprecated,
    reason = "`SymbolInformation` has no constructor, so building one names its deprecated field"
)]

use alloc::sync::Arc;

use tower_lsp::lsp_types::{
    DocumentSymbol, DocumentSymbolResponse, Location, SymbolInformation, Url,
};

use crate::document::DocumentState;
use crate::providers::document_symbol;

const MAX_RESULTS: usize = 1000;

pub(crate) fn workspace_symbols(
    documents: &[(Url, Arc<DocumentState>)],
    query: &str,
) -> Vec<SymbolInformation> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();
    for (uri, document) in documents {
        let DocumentSymbolResponse::Nested(symbols) = document_symbol::document_symbols(document)
        else {
            continue;
        };
        flatten(&symbols, uri, None, &query_lower, &mut results);
        if results.len() >= MAX_RESULTS {
            results.truncate(MAX_RESULTS);
            break;
        }
    }
    results
}

fn flatten(
    symbols: &[DocumentSymbol],
    uri: &Url,
    container: Option<&str>,
    query_lower: &str,
    results: &mut Vec<SymbolInformation>,
) {
    for symbol in symbols {
        if results.len() >= MAX_RESULTS {
            return;
        }
        if query_lower.is_empty() || symbol.name.to_lowercase().contains(query_lower) {
            results.push(SymbolInformation {
                name: symbol.name.clone(),
                kind: symbol.kind,
                tags: symbol.tags.clone(),
                deprecated: None,
                location: Location { uri: uri.clone(), range: symbol.range },
                container_name: container.map(str::to_owned),
            });
        }
        if let Some(children) = &symbol.children {
            flatten(children, uri, Some(&symbol.name), query_lower, results);
        }
    }
}
