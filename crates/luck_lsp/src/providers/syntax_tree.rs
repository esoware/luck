//! Custom `luck/syntaxTree` request. Returns a debug pretty-print of the
//! parsed AST for the currently-open document. The VS Code extension
//! opens the response in a side panel.

use crate::backend::DocumentState;

#[must_use]
pub fn syntax_tree(doc: &DocumentState) -> String {
    format!("{:#?}", doc.parsed.block)
}
