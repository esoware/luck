use luck_lexer::lex;
use luck_token::{Comment, CommentKind, CommentPosition, LuaVersion, SourceError, Span, TokenKind};

use crate::common::lex_lua51;

#[test]
fn line_comment_after_a_token_trails_it() {
    let result = lex_lua51("x -- comment");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::Line);
    assert_eq!(result.comments[0].position, CommentPosition::Trailing);
}

#[test]
fn block_comment_without_line_break_is_single_line() {
    let result = lex_lua51("x --[[ block ]]");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::SingleLineBlock);
}

#[test]
fn block_comment_with_line_break_is_multi_line() {
    let result = lex_lua51("x --[[ block\n comment ]]");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::MultiLineBlock);
}

#[test]
fn block_comment_closes_at_its_own_level() {
    let result = lex_lua51("x --[=[ block ]] ]=]");
    assert_eq!(result.errors, Vec::<SourceError>::new());
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::SingleLineBlock);
    assert_eq!(result.comments[0].span, Span::new(2, 20));
}

#[test]
fn unterminated_block_comment_is_error() {
    let result = lex_lua51("--[[ oops");
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("unterminated"));
    assert_eq!(result.comments, Vec::<Comment>::new());
}

#[test]
fn shebang_leads_the_first_token() {
    let result = lex_lua51("#!/usr/bin/env lua\nlocal x");
    assert_eq!(
        result.comments,
        vec![Comment {
            span: Span::new(0, 18),
            attached_to: 19,
            kind: CommentKind::Shebang,
            position: CommentPosition::Leading,
            is_preceded_by_newline: false,
            is_followed_by_newline: true,
        }]
    );
}

#[test]
fn bom_then_shebang_is_skipped() {
    let result = lex("\u{FEFF}#!/usr/bin/lua\nprint(1)", LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert_eq!(result.tokens[0].kind, TokenKind::Identifier("print".into()));
}

#[test]
fn hash_after_the_first_byte_is_the_length_operator() {
    let result = lex_lua51("x = #t");
    assert_eq!(result.comments, Vec::<Comment>::new());
    assert!(result.tokens.iter().any(|token| token.kind == TokenKind::Hash));
}

#[test]
fn trailing_comment_attaches_to_the_previous_token() {
    let result = lex_lua51("local x -- trailing");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Trailing);
    assert_eq!(comment.attached_to, result.tokens[1].span.start);
    assert!(!comment.is_preceded_by_newline);
}

#[test]
fn leading_comment_attaches_to_the_next_token() {
    let result = lex_lua51("-- leading\nlocal x");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
    let local_token = &result.tokens[0];
    assert_eq!(comment.attached_to, local_token.span.start);
}

#[test]
fn comment_after_newline_is_leading() {
    let result = lex_lua51("x\n-- comment\ny");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
    assert!(comment.is_preceded_by_newline);
}

#[test]
fn consecutive_leading_comments_all_lead_the_next_token() {
    let result = lex_lua51("-- first\n-- second\nlocal x");
    assert_eq!(result.comments.len(), 2);
    for comment in &result.comments {
        assert_eq!(comment.position, CommentPosition::Leading);
        assert_eq!(comment.attached_to, result.tokens[0].span.start);
    }
}

#[test]
fn comment_before_the_first_token_on_its_line_leads_it() {
    // The file start counts as a newline.
    let result = lex_lua51("--[[ a ]] x");
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
    assert_eq!(comment.attached_to, 10);
    assert!(comment.is_preceded_by_newline);
    assert!(!comment.is_followed_by_newline);
}

#[test]
fn comment_at_end_of_file_leads_eof() {
    let result = lex_lua51("x\n-- end");
    let eof = result.tokens.last().expect("an EOF token");
    assert_eq!(eof.kind, TokenKind::Eof);
    assert_eq!(result.comments[0].position, CommentPosition::Leading);
    assert_eq!(result.comments[0].attached_to, eof.span.start);
    assert!(result.comments[0].is_followed_by_newline);
}

#[test]
fn line_comment_is_followed_by_newline() {
    let result = lex_lua51("x -- trailing\ny");
    assert_eq!(result.comments.len(), 1);
    assert!(result.comments[0].is_followed_by_newline);
}

#[test]
fn block_comment_is_followed_by_newline_only_across_spaces_and_tabs() {
    for (source, is_followed_by_newline) in
        [("x --[[ a ]] \t\ny", true), ("x --[[ a ]] \t", true), ("x --[[ a ]] y", false)]
    {
        let result = lex_lua51(source);
        assert_eq!(result.comments[0].is_followed_by_newline, is_followed_by_newline, "{source:?}");
    }
}

#[test]
fn comment_after_a_multi_line_block_comment_leads_the_next_token() {
    let result = lex_lua51("x --[[\n]] -- c\ny");
    assert_eq!(result.comments.len(), 2);
    assert_eq!(result.comments[0].position, CommentPosition::Trailing);
    assert_eq!(result.comments[0].attached_to, 0);
    assert!(!result.comments[0].is_preceded_by_newline);
    assert_eq!(result.comments[1].position, CommentPosition::Leading);
    assert_eq!(result.comments[1].attached_to, 15);
    assert!(result.comments[1].is_preceded_by_newline);
}

#[test]
fn single_line_block_comment_keeps_the_next_comment_trailing() {
    let result = lex_lua51("x --[[ a ]] -- c\ny");
    assert_eq!(result.comments[1].position, CommentPosition::Trailing);
    assert!(!result.comments[1].is_preceded_by_newline);
}
