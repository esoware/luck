//! Custom `luck/syntaxTree` request: a debug pretty-print of the document's
//! parsed AST. The VS Code extension opens it beside the editor.

use crate::document::DocumentState;

pub(crate) fn syntax_tree(document: &DocumentState) -> String {
    format!("{:#?}", document.parsed.block)
}
