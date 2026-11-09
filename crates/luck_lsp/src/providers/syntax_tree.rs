//! Custom `luck/syntaxTree` request: a debug pretty-print of the document's
//! parsed AST. The VS Code extension opens it beside the editor.

use core::fmt::{self, Write as _};

use crate::document::DocumentState;

/// How much of a dump to keep. Pretty-printing indents each nesting level, so
/// a long `a + b + ...` chain's dump grows with the square of its length.
const MAX_DUMP_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn syntax_tree(document: &DocumentState) -> String {
    let mut dump = BoundedDump { text: String::new() };
    #[expect(clippy::use_debug, reason = "this dump's format is the AST's `Debug` output")]
    // The only error is the limit, which ends the dump with its marker.
    let _unused = write!(dump, "{:#?}", document.parsed.block);
    dump.text
}

/// A `fmt` sink that stops accepting text once [`MAX_DUMP_BYTES`] is reached.
struct BoundedDump {
    text: String,
}

impl fmt::Write for BoundedDump {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.text.len().saturating_add(s.len()) > MAX_DUMP_BYTES {
            self.text.push_str("\n... the rest of the tree is cut off here");
            return Err(fmt::Error);
        }
        self.text.push_str(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;

    use super::*;

    #[test]
    fn a_long_chain_dumps_within_the_bound() {
        let chain = vec!["1"; 50_000].join("+");
        let text = format!("local x = {chain}\n");
        let document = DocumentState::new(text, 1, LuaTarget::Lua54, FxHashSet::default());
        let dump = syntax_tree(&document);
        assert!(dump.len() <= MAX_DUMP_BYTES.saturating_add(64), "{}", dump.len());
        assert!(dump.ends_with("cut off here"), "the dump says it stopped early");
    }
}
