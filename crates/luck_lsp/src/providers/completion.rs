//! Luau's own autocomplete, with each entry's type and the comment a
//! definition file has above what it names.

use ls_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, CompletionResponse, Documentation,
    MarkupContent, MarkupKind, Position,
};
use luck_type_checker::{Completion, CompletionKind};

use crate::document::DocumentState;
use crate::type_check::Types;

pub(crate) fn completion(
    document: &DocumentState,
    position: Position,
    types: &mut Types<'_>,
) -> CompletionResponse {
    let completions = types.autocomplete(document.offset(position));
    let items = completions
        .into_iter()
        .map(|completion| {
            let Completion { name, kind, ty, is_deprecated, insert_text, declaration } = completion;
            let documentation = types.documentation(&declaration).map(|value| {
                Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value })
            });
            CompletionItem {
                kind: Some(match kind {
                    CompletionKind::Property => CompletionItemKind::PROPERTY,
                    CompletionKind::Binding => CompletionItemKind::VARIABLE,
                    CompletionKind::Keyword | CompletionKind::HotComment => {
                        CompletionItemKind::KEYWORD
                    }
                    CompletionKind::String => CompletionItemKind::VALUE,
                    CompletionKind::Type => CompletionItemKind::CLASS,
                    CompletionKind::Module => CompletionItemKind::MODULE,
                    CompletionKind::GeneratedFunction => CompletionItemKind::FUNCTION,
                    CompletionKind::RequirePath => CompletionItemKind::FILE,
                }),
                detail: ty,
                documentation,
                deprecated: Some(is_deprecated),
                // Deprecated entries sink below live ones.
                sort_text: Some(format!("{}{name}", if is_deprecated { '2' } else { '1' })),
                tags: is_deprecated.then(|| vec![CompletionItemTag::DEPRECATED]),
                insert_text,
                label: name,
                ..CompletionItem::default()
            }
        })
        .collect();
    CompletionResponse::Array(items)
}
