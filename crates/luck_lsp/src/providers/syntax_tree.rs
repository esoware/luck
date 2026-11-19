use core::fmt::{self, Write as _};
use core::mem;

use crate::document::DocumentState;

/// How much of a dump to keep. Pretty-printing puts every field of every node
/// on an indented line of its own, so a large file's dump runs to many times
/// its size.
const MAX_DUMP_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn syntax_tree(document: &DocumentState) -> String {
    let mut dump = PrettyDump::default();
    #[expect(clippy::use_debug, reason = "this dump's format is the AST's `Debug` output")]
    // The only error is the limit, which ends the dump with its marker.
    let _unused = write!(dump, "{:?}", document.parse().chunk);
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
        if self.text.len().saturating_add(piece.len()) > MAX_DUMP_BYTES {
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
        self.depth = self.depth.saturating_add(1);
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
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    use luck_ast::MAX_NESTING_DEPTH;
    use luck_core::LuaTarget;
    use luck_testgen::{
        collect_fixture_files, detect_version, generate_full, on_the_smallest_stack,
    };
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::document;

    const fn target(version: LuaVersion) -> LuaTarget {
        match version {
            LuaVersion::Lua51 => LuaTarget::Lua51,
            LuaVersion::Lua52 => LuaTarget::Lua52,
            LuaVersion::Lua53 => LuaTarget::Lua53,
            LuaVersion::Lua54 => LuaTarget::Lua54,
            LuaVersion::Lua55 => LuaTarget::Lua55,
            LuaVersion::Luau => LuaTarget::Luau,
        }
    }

    #[test]
    fn a_dump_lays_the_tree_out_as_pretty_debug_does() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let mut sources = Vec::new();
        for path in collect_fixture_files(&root).expect("fixture tree is readable") {
            if matches!(path.extension().and_then(OsStr::to_str), Some("lua" | "luau")) {
                let relative = path.strip_prefix(&root).expect("fixture is under the root");
                let text = fs::read_to_string(&path).expect("fixture is readable");
                sources.push((detect_version(relative), text));
            }
        }
        assert!(!sources.is_empty(), "no fixtures found under {}", root.display());
        for version in LuaVersion::ALL {
            sources.extend((0..20).map(|seed| (version, generate_full(seed, version, 25))));
        }
        for (version, text) in sources {
            let document = document(&text, target(version));
            let expected = format!("{:#?}", document.parse().chunk);
            if expected.len() <= MAX_DUMP_BYTES {
                assert_eq!(syntax_tree(&document), expected, "{version:?}\n{text}");
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
        let levels = MAX_NESTING_DEPTH.saturating_sub(1).div_euclid(2);
        let source = format!(
            "{}local innermost = 1{}",
            "local function f() ".repeat(levels),
            " end".repeat(levels)
        );
        on_the_smallest_stack([(LuaVersion::Lua54, source)], |text, _| {
            let document = document(text, LuaTarget::Lua54);
            assert!(document.parse().errors.is_empty(), "{:?}", document.parse().errors);
            let dump = syntax_tree(&document);
            assert!(dump.len() <= MAX_DUMP_BYTES.saturating_add(64), "{}", dump.len());
            assert!(dump.ends_with("cut off here"), "the dump says it stopped early");
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
