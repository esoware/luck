//! Only open documents are searched, since a closed file's record keeps
//! what other files see of it, not its outline.

#![expect(
    deprecated,
    reason = "`SymbolInformation` has no constructor, so building one names its deprecated field"
)]

use ls_types::{DocumentSymbol, Location, SymbolInformation, Uri};

use crate::document::DocumentState;

const MAX_RESULTS: usize = 1000;

/// Each version's outline is built once, so a query typed a letter at a time
/// only filters.
pub(crate) fn workspace_symbols<'document, I>(documents: I, query: &str) -> Vec<SymbolInformation>
where
    I: IntoIterator<Item = (&'document Uri, &'document DocumentState)>,
{
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();
    for (uri, document) in documents {
        flatten(document.symbols(), uri, None, &query_lower, &mut results);
        if results.len() >= MAX_RESULTS {
            results.truncate(MAX_RESULTS);
            break;
        }
    }
    results
}

fn flatten(
    symbols: &[DocumentSymbol],
    uri: &Uri,
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
