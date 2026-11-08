//! One module per LSP feature, each working from the cached `DocumentState`.
//! `cursor` holds the position lookups they share.

pub(crate) mod code_action;
pub(crate) mod completion;
pub(crate) mod cursor;
pub(crate) mod definition;
pub(crate) mod document_link;
pub(crate) mod document_symbol;
pub(crate) mod folding;
pub(crate) mod highlights;
pub(crate) mod hover;
pub(crate) mod references;
pub(crate) mod rename;
pub(crate) mod selection_range;
pub(crate) mod semantic_tokens;
pub(crate) mod signature_help;
pub(crate) mod syntax_tree;
pub(crate) mod workspace_symbol;
