use std::fmt::{self, Write as _};
use std::mem;

use crate::document::DocumentState;

/// How much of a dump to keep. Pretty-printing puts every field of every node
/// on an indented line of its own, so a large file's dump runs to many times
/// its size.
const MAX_DUMP_BYTES: usize = 8 * 1024 * 1024;

#[expect(clippy::use_debug, reason = "this dump's format is the AST's `Debug` output")]
pub(crate) fn syntax_tree(document: &DocumentState) -> String {
    let mut dump = PrettyDump::default();
    // The only error is the limit, which ends the dump with its marker.
    let _ = write!(dump, "{:?}", document.parse().chunk);
    dump.text
}

/// Lays compact `Debug` output out as `{:#?}` does. `{:#?}` writes through
/// one more writer for each level open, so a line there costs time in
/// proportion to its depth, and here in proportion to its length.
#[derive(Default)]
struct PrettyDump {
    text: String,
    depth: usize,
    /// The quote of the string or char literal being copied.
    quote: Option<char>,
    is_escaped: bool,
    /// A space, `(`, or `[`, held until the next character shows whether it
    /// opens or closes something.
    held: Option<char>,
    /// Whether a space that compact output writes after `{` or `,` comes
    /// next.
    is_after_separator: bool,
    /// Whether the dump reached the bound, and holds the marker that says so.
    is_cut_off: bool,
}

impl PrettyDump {
    fn push(&mut self, piece: &str) {
        if self.is_cut_off {
            return;
        }
        if self.text.len() + piece.len() > MAX_DUMP_BYTES {
            self.text.push_str("\n... the rest of the tree is cut off here");
            self.is_cut_off = true;
            return;
        }
        self.text.push_str(piece);
    }

    fn push_char(&mut self, c: char) {
        self.push(c.encode_utf8(&mut [0; 4]));
    }

    fn new_line(&mut self) {
        self.push("\n");
        for _ in 0..self.depth {
            self.push("    ");
        }
    }

    fn open(&mut self, opener: &str) {
        self.push(opener);
        self.depth += 1;
        self.new_line();
    }

    fn close(&mut self, closer: char) {
        self.push(",");
        self.depth = self.depth.saturating_sub(1);
        self.new_line();
        self.push_char(closer);
    }

    fn lay_out(&mut self, c: char) {
        if let Some(quote) = self.quote {
            let is_escaped = mem::take(&mut self.is_escaped);
            if !is_escaped && c == '\\' {
                self.is_escaped = true;
            }
            if !is_escaped && c == quote {
                self.quote = None;
            }
            self.push_char(c);
            return;
        }
        if mem::take(&mut self.is_after_separator) && c == ' ' {
            return;
        }
        match (self.held.take(), c) {
            (Some(' '), '{') => {
                self.is_after_separator = true;
                self.open(" {");
            }
            (Some(' '), '}') => self.close('}'),
            (Some('('), ')') => self.push("()"),
            (Some('['), ']') => self.push("[]"),
            (held, _) => {
                match held {
                    Some(' ') => self.push(" "),
                    Some(opener) => self.open(opener.encode_utf8(&mut [0; 4])),
                    None => {}
                }
                match c {
                    ' ' | '(' | '[' => self.held = Some(c),
                    ',' => {
                        self.is_after_separator = true;
                        self.push(",");
                        self.new_line();
                    }
                    ')' | ']' => self.close(c),
                    '"' | '\'' => {
                        self.quote = Some(c);
                        self.push_char(c);
                    }
                    _ => self.push_char(c),
                }
            }
        }
    }
}

impl fmt::Write for PrettyDump {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.lay_out(c);
        }
        if self.is_cut_off { Err(fmt::Error) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use luck_ast::MAX_NESTING_DEPTH;
    use luck_testgen::{generate_full_grammar, luau_fixtures, on_the_smallest_stack};

    use super::*;
    use crate::fixtures::document;

    #[test]
    fn a_dump_lays_the_tree_out_as_pretty_debug_does() {
        let mut sources: Vec<String> =
            luau_fixtures().into_iter().map(|fixture| fixture.source).collect();
        sources.extend((0..20).map(|seed| generate_full_grammar(seed, 25)));
        for text in sources {
            let document = document(&text);
            let expected = format!("{:#?}", document.parse().chunk);
            if expected.len() <= MAX_DUMP_BYTES {
                assert_eq!(syntax_tree(&document), expected, "{text}");
            }
        }
    }

    /// One tree at the cap, which nests statements and function bodies.
    ///
    /// Every level indents its lines further, so the dump reaches the bound
    /// long before the innermost node, however deep the tree.
    #[test]
    fn a_tree_at_the_cap_dumps_within_the_bound_on_the_smallest_stack() {
        // The chunk is a level, and each function a body and a block.
        let levels = (MAX_NESTING_DEPTH - 1) / 2;
        let source = format!(
            "{}local innermost = 1{}",
            "local function f() ".repeat(levels),
            " end".repeat(levels)
        );
        on_the_smallest_stack([source], |text| {
            let document = document(text);
            assert!(document.parse().errors.is_empty(), "{:?}", document.parse().errors);
            let dump = syntax_tree(&document);
            assert!(dump.len() <= MAX_DUMP_BYTES + 64, "{}", dump.len());
            assert!(dump.ends_with("cut off here"), "the dump says it stopped early");
        });
    }

    #[test]
    fn large_files_dump_within_the_bound() {
        let chain = vec!["1"; 2000].join("+");
        let text = format!("local x = {chain}\n").repeat(40);
        let document = document(&text);
        let dump = syntax_tree(&document);
        assert!(dump.len() <= MAX_DUMP_BYTES + 64, "{}", dump.len());
        assert!(dump.ends_with("cut off here"), "the dump says it stopped early");
    }
}
