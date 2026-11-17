use core::fmt::{self, Write as _};

use crate::document::DocumentState;

/// How much of a dump to keep. Pretty-printing puts every field of every node
/// on an indented line of its own, so a large file's dump runs to many times
/// its size.
const MAX_DUMP_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn syntax_tree(document: &DocumentState) -> String {
    let mut dump = BoundedDump { text: String::new() };
    #[expect(clippy::use_debug, reason = "this dump's format is the AST's `Debug` output")]
    // The only error is the limit, which ends the dump with its marker.
    let _unused = write!(dump, "{:#?}", document.parse().block);
    dump.text
}

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
    use luck_ast::MAX_NESTING_DEPTH;
    use luck_core::LuaTarget;
    use luck_testgen::on_the_smallest_stack;
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::document;

    /// One tree at the cap, which nests statements and function bodies.
    ///
    /// `{:#?}` writes through a writer per level open, so each such dump
    /// takes seconds, and the dump of this one ends within the bound.
    #[test]
    fn a_tree_at_the_cap_dumps_on_the_smallest_stack() {
        // The chunk is a level, and each function a body and a block.
        let levels = MAX_NESTING_DEPTH.saturating_sub(1).div_euclid(2);
        let source = format!(
            "{}local innermost = 1{}",
            "local function f() ".repeat(levels),
            " end".repeat(levels)
        );
        on_the_smallest_stack([(LuaVersion::Lua54, source)], |text, _| {
            let document = document(text, LuaTarget::Lua54);
            assert!(document.parse().errors.is_empty(), "{:?}", document.parse().errors);
            assert!(syntax_tree(&document).contains("innermost"), "the dump got to the bottom");
        });
    }

    #[test]
    fn large_files_dump_within_the_bound() {
        let chain = vec!["1"; 2000].join("+");
        let text = format!("local x = {chain}\n").repeat(40);
        let document = document(&text, LuaTarget::Lua54);
        let dump = syntax_tree(&document);
        assert!(dump.len() <= MAX_DUMP_BYTES.saturating_add(64), "{}", dump.len());
        assert!(dump.ends_with("cut off here"), "the dump says it stopped early");
    }
}
