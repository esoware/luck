use core::fmt::{self, Write as _};
use core::mem;

use crate::document::DocumentState;

/// How much of a dump to keep. Pretty-printing puts every field of every node
/// on an indented line of its own, so a large file's dump runs to many times
/// its size.
const MAX_DUMP_BYTES: usize = 8 * 1024 * 1024;

/// The AST's `{:#?}` form, built from its one-line `{:?}` form as it's
/// written.
///
/// `{:#?}` indents through a writer per level open, so a tree nested to the
/// depth cap would cost hundreds of writers per byte.
pub(crate) fn syntax_tree(document: &DocumentState) -> String {
    let mut dump = PrettyDump::default();
    #[expect(clippy::use_debug, reason = "this dump's format is the AST's `Debug` output")]
    // The only error is the limit, which ends the dump with its marker.
    let _unused = write!(dump, "{:?}", document.parsed.block);
    dump.text
}

/// Puts each field and element of a one-line `Debug` form on a line of its
/// own, ending in a comma and indented a level per bracket open, as `{:#?}`
/// does.
#[derive(Default)]
struct PrettyDump {
    text: String,
    depth: usize,
    quoting: Quoting,
    /// A `(` or `[` that holds nothing when its closer comes next.
    pending_open: Option<char>,
    space: Space,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Quoting {
    #[default]
    Outside,
    Inside,
    /// Right after a `\` inside a quoted string.
    Escaped,
}

/// What becomes of a space the one-line form writes.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Space {
    #[default]
    None,
    /// One that separates nothing when a `}` comes next.
    Pending,
    /// The one after `,` or `{`, which a line break replaces.
    Dropped,
}

impl PrettyDump {
    fn push(&mut self, c: char) {
        match (self.quoting, c) {
            (Quoting::Escaped, _) => {
                self.text.push(c);
                self.quoting = Quoting::Inside;
                return;
            }
            (Quoting::Inside, '\\') => {
                self.text.push(c);
                self.quoting = Quoting::Escaped;
                return;
            }
            (Quoting::Inside, '"') => {
                self.text.push(c);
                self.quoting = Quoting::Outside;
                return;
            }
            (Quoting::Inside, _) => {
                self.text.push(c);
                return;
            }
            (Quoting::Outside, _) => {}
        }
        if let Some(open) = self.pending_open.take() {
            self.text.push(open);
            if matches!((open, c), ('(', ')') | ('[', ']')) {
                self.text.push(c);
                return;
            }
            self.open_line();
        }
        match mem::take(&mut self.space) {
            Space::Dropped if c == ' ' => return,
            Space::Pending if c != '}' => self.text.push(' '),
            Space::None | Space::Pending | Space::Dropped => {}
        }
        match c {
            ' ' => self.space = Space::Pending,
            '(' | '[' => self.pending_open = Some(c),
            '{' => {
                self.text.push('{');
                self.open_line();
                self.space = Space::Dropped;
            }
            ')' | ']' | '}' => {
                self.text.push(',');
                self.depth = self.depth.saturating_sub(1);
                self.new_line();
                self.text.push(c);
            }
            ',' => {
                self.text.push(',');
                self.new_line();
                self.space = Space::Dropped;
            }
            '"' => {
                self.text.push(c);
                self.quoting = Quoting::Inside;
            }
            _ => self.text.push(c),
        }
    }

    fn open_line(&mut self) {
        self.depth = self.depth.saturating_add(1);
        self.new_line();
    }

    fn new_line(&mut self) {
        self.text.push('\n');
        for _ in 0..self.depth {
            self.text.push_str("    ");
        }
    }
}

impl fmt::Write for PrettyDump {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.push(c);
        }
        if self.text.len() > MAX_DUMP_BYTES {
            self.text.push_str("\n... the rest of the tree is cut off here");
            return Err(fmt::Error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;

    use super::*;

    #[test]
    fn large_files_dump_within_the_bound() {
        let chain = vec!["1"; 2000].join("+");
        let text = format!("local x = {chain}\n").repeat(40);
        let document = DocumentState::new(text, 1, LuaTarget::Lua54, FxHashSet::default());
        let dump = syntax_tree(&document);
        assert!(dump.len() <= MAX_DUMP_BYTES.saturating_add(4096), "{}", dump.len());
        assert!(dump.ends_with("cut off here"), "the dump says it stopped early");
    }

    #[test]
    fn the_dump_is_the_trees_pretty_debug_form() {
        for (text, target) in [
            (
                r#"local x, y = 1, "a \"b\" {c}, (d)"
return x"#,
                LuaTarget::Lua54,
            ),
            ("local t = {} f() function t.m(a, ...) return end", LuaTarget::Lua54),
            (
                "\
type X<T> = {[string]: (T?) -> ()}
return `a{b}c`",
                LuaTarget::Luau,
            ),
        ] {
            let document = DocumentState::new(text.to_owned(), 1, target, FxHashSet::default());
            let expected = format!("{:#?}", document.parsed.block);
            assert_eq!(syntax_tree(&document), expected, "{text:?}");
        }
    }
}
