//! LSP provider implementations. Each module backs one LSP capability and
//! consumes the cached `DocumentState` (text + parsed AST + line index).

// Several providers parameterize switches and helpers that aren't all
// wired up yet - keep the surface for future plumbing without warnings.

pub mod code_action;
pub mod completion;
pub mod cursor;
pub mod definition;
pub mod document_link;
pub mod document_symbol;
pub mod folding;
pub mod highlights;
pub mod hover;
pub mod inlay_hints;
pub mod references;
pub mod rename;
pub mod selection_range;
pub mod semantic_tokens;
pub mod signature_help;
pub mod syntax_tree;
pub mod workspace_symbol;
