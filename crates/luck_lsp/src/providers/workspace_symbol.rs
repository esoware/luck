//! Workspace symbols. Flattens each open document's outline and filters
//! by a case-insensitive substring query. Only open documents are
//! searched - there is no on-disk index - which matches how the rest of
//! the server scopes its analysis.

#![allow(deprecated)]

use tower_lsp::lsp_types::{
    DocumentSymbol, DocumentSymbolResponse, Location, SymbolInformation, Url,
};

use crate::backend::DocumentState;
use crate::providers::document_symbol;

const MAX_RESULTS: usize = 1000;

#[must_use]
pub fn workspace_symbols(
    documents: &[(Url, DocumentState)],
    query: &str,
) -> Vec<SymbolInformation> {
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();
    for (uri, doc) in documents {
        let DocumentSymbolResponse::Nested(symbols) = document_symbol::document_symbols(doc) else {
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
    out: &mut Vec<SymbolInformation>,
) {
    for symbol in symbols {
        if out.len() >= MAX_RESULTS {
            return;
        }
        if query_lower.is_empty() || symbol.name.to_lowercase().contains(query_lower) {
            out.push(SymbolInformation {
                name: symbol.name.clone(),
                kind: symbol.kind,
                tags: symbol.tags.clone(),
                deprecated: None,
                location: Location {
                    uri: uri.clone(),
                    range: symbol.range,
                },
                container_name: container.map(str::to_string),
            });
        }
        if let Some(children) = &symbol.children {
            flatten(children, uri, Some(&symbol.name), query_lower, out);
        }
    }
}
