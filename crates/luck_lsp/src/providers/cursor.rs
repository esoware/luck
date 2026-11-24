use luck_ast::{Name, StringLiteral};
use luck_semantic::{NodeId, SymbolId};
use luck_token::{CommentKind, Span, TokenKind};

use crate::document::DocumentState;

/// The name at `offset`, either end included, and its node, or `None` where
/// the innermost node there is something else or a name recovery made up.
pub(crate) fn name_at(document: &DocumentState, offset: u32) -> Option<(NodeId, &Name<'_>)> {
    let nodes = document.nodes();
    let id = nodes.node_at(offset)?;
    let name = nodes.kind(id).as_name().filter(|name| name.written().is_some())?;
    Some((id, name))
}

/// The local the name at `offset` declares or refers to, and that name's
/// span.
///
/// The implicit `self` and Lua 5.1 `arg` have no name that declares them, so
/// only their references find them.
pub(crate) fn local_at(document: &DocumentState, offset: u32) -> Option<(SymbolId, Span)> {
    let (_, name) = name_at(document, offset)?;
    let tree = document.scoping();
    let symbol = tree.symbol_of(name).or_else(|| tree.resolved_symbol_of(name))?;
    Some((symbol, name.span))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextKind {
    Code,
    Comment,
    String,
}

/// Text that a closing delimiter ends counts up to it, and text a line break
/// or the end of the file cut off counts through its end, where typing goes
/// on inside it.
pub(crate) fn text_kind_at(document: &DocumentState, offset: u32) -> TextKind {
    let parse = document.parse();
    let comments = &parse.comments;
    let tokens = document.tokens();
    let index = comments.partition_point(|comment| comment.span.end < offset);
    let is_in_comment = comments.get(index).is_some_and(|comment| match comment.kind {
        CommentKind::Line | CommentKind::Shebang => {
            comment.span.start < offset && offset <= comment.span.end
        }
        CommentKind::SingleLineBlock | CommentKind::MultiLineBlock => {
            let is_last = index + 1 == comments.len();
            let is_cut_off = is_last && tokens.last().is_some_and(|eof| eof.is_unterminated);
            is_inside(comment.span, offset, is_cut_off)
        }
    });
    if is_in_comment {
        return TextKind::Comment;
    }
    let enclosing = tokens.get(tokens.partition_point(|token| token.span.end < offset));
    let is_in_string = enclosing.is_some_and(|token| {
        matches!(
            token.kind,
            TokenKind::StringLiteral
                | TokenKind::InterpolatedStringBegin
                | TokenKind::InterpolatedStringMiddle
                | TokenKind::InterpolatedStringEnd
                | TokenKind::InterpolatedStringSimple
        ) && is_inside(token.span, offset, token.is_unterminated)
    });
    if is_in_string { TextKind::String } else { TextKind::Code }
}

/// Whether `offset` is inside the string token or block comment at `span`:
/// past its first byte, and before its end unless a line break or the end of
/// the text cut it off there.
pub(crate) const fn is_inside(span: Span, offset: u32, is_cut_off: bool) -> bool {
    span.start < offset && (offset < span.end || (offset == span.end && is_cut_off))
}

/// Whether the string token starting where `literal` does runs to the end of
/// its line or of the source without its closing delimiter.
pub(crate) fn is_unterminated(document: &DocumentState, literal: &StringLiteral<'_>) -> bool {
    let tokens = document.tokens();
    tokens
        .binary_search_by_key(&literal.span.start, |token| token.span.start)
        .is_ok_and(|i| tokens[i].is_unterminated)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    /// The kind of text at each `|` in `marked`, which the text holds without
    /// them.
    fn kinds_at_marks(marked: &str, target: LuaTarget) -> Vec<TextKind> {
        let text = marked.replace('|', "");
        let document = document(&text, target);
        let mut offsets = Vec::new();
        for (index, (position, _)) in marked.match_indices('|').enumerate() {
            offsets.push(u32::try_from(position - index).expect("a small offset"));
        }
        offsets.into_iter().map(|offset| text_kind_at(&document, offset)).collect()
    }

    #[test]
    fn a_string_cut_off_by_its_line_holds_its_end() {
        use TextKind::{Code, Comment, String};
        for (marked, target, expected) in [
            ("x = `{|y}tail|", LuaTarget::Luau, vec![Code, String]),
            ("x = `{y}ta|il`|", LuaTarget::Luau, vec![String, Code]),
            ("x = `|tail|", LuaTarget::Luau, vec![String, String]),
            ("x = `|", LuaTarget::Luau, vec![String]),
            ("x = `a|`|", LuaTarget::Luau, vec![String, Code]),
            ("x = \"a|bc|", LuaTarget::Lua54, vec![String, String]),
            ("x = \"abc\"|", LuaTarget::Lua54, vec![Code]),
            (
                r#"x = "a\"|
y = 1|"#,
                LuaTarget::Lua54,
                vec![String, Code],
            ),
            ("x = [[a|", LuaTarget::Lua54, vec![String]),
            ("x = [[a]]|", LuaTarget::Lua54, vec![Code]),
            ("--[[ a|", LuaTarget::Lua54, vec![Comment]),
            ("--[[ a ]]|", LuaTarget::Lua54, vec![Code]),
            ("x = 1 |--[==[ a ]]|", LuaTarget::Lua54, vec![Code, Comment]),
            (
                "\
--[[ a
x = \"b|\"
|",
                LuaTarget::Lua54,
                vec![Comment, Comment],
            ),
            ("--[[ a [[ b ]]|", LuaTarget::Lua51, vec![Code]),
            ("--[[]]|", LuaTarget::Lua54, vec![Code]),
            ("--[[]|", LuaTarget::Lua54, vec![Comment]),
            ("--[==[ ]=] ]==]|", LuaTarget::Lua54, vec![Code]),
        ] {
            assert_eq!(kinds_at_marks(marked, target), expected, "{marked:?}");
        }
    }

    #[test]
    fn a_local_is_found_from_its_declaration_and_every_reference() {
        let text = "\
local value = 1
do local value = value end
return value, other";
        let document = document(text, LuaTarget::Lua54);
        let symbol_at = |offset| local_at(&document, offset).map(|(symbol, _)| symbol.index());
        // The outer `value` is declared at 6, read at 33, and returned at 50.
        assert_eq!(symbol_at(6), Some(0));
        assert_eq!(symbol_at(33), Some(0), "the inner initializer reads the outer local");
        assert_eq!(symbol_at(25), Some(1));
        assert_eq!(symbol_at(55), Some(0), "a cursor just past a name is on it");
        assert_eq!(symbol_at(57), None, "a global is no local");
        assert_eq!(symbol_at(0), None, "a keyword is no name");
    }
}
