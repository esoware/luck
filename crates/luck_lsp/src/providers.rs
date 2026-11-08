//! One module per LSP feature, each working from the cached `DocumentState`.
//! `cursor` holds the position lookups they share, and `stdlib_render` the
//! stdlib rendering.

pub(crate) mod code_action;
pub(crate) mod completion;
mod cursor;
pub(crate) mod definition;
pub(crate) mod diagnostics;
pub(crate) mod document_highlight;
pub(crate) mod document_link;
pub(crate) mod document_symbol;
pub(crate) mod folding_range;
pub(crate) mod hover;
pub(crate) mod references;
pub(crate) mod rename;
pub(crate) mod selection_range;
pub(crate) mod semantic_tokens;
pub(crate) mod signature_help;
mod stdlib_render;
pub(crate) mod syntax_tree;
pub(crate) mod workspace_symbol;
