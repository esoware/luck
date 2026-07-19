use luck_token::*;

use crate::common::lex51;

#[test]
fn line_comment() {
    let result = lex51("x -- comment");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::Line);
    assert_eq!(result.comments[0].position, CommentPosition::Trailing);
}

#[test]
fn block_comment_single_line() {
    let result = lex51("x --[[ block ]]");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::SingleLineBlock);
}

#[test]
fn block_comment_multi_line() {
    let result = lex51("x --[[ block\n comment ]]");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::MultiLineBlock);
}

#[test]
fn block_comment_with_level() {
    let result = lex51("x --[=[ block ]=]");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::SingleLineBlock);
}

#[test]
fn shebang_comment() {
    let result = lex51("#!/usr/bin/env lua\nlocal x");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::Shebang);
    assert_eq!(result.comments[0].position, CommentPosition::Leading);
}

#[test]
fn hash_not_shebang_midfile() {
    // # at a non-zero position is the length operator, not shebang
    let result = lex51("x = #t");
    assert!(result.comments.is_empty());
    let ks = result.tokens.iter().map(|t| &t.kind).collect::<Vec<_>>();
    assert!(ks.contains(&&TokenKind::Hash));
}

#[test]
fn trailing_comment_attachment() {
    let result = lex51("local x -- trailing");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Trailing);
    assert!(!comment.preceded_by_newline);
}

#[test]
fn leading_comment_attachment() {
    let result = lex51("-- leading\nlocal x");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
    // attached_to should be the start of `local`
    let local_token = &result.tokens[0];
    assert_eq!(comment.attached_to, local_token.span.start);
}

#[test]
fn comment_after_newline_is_leading() {
    let result = lex51("x\n-- comment\ny");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
    assert!(comment.preceded_by_newline);
}

#[test]
fn multiple_comments_attachment() {
    let result = lex51("-- first\n-- second\nlocal x");
    assert_eq!(result.comments.len(), 2);
    for comment in &result.comments {
        assert_eq!(comment.position, CommentPosition::Leading);
    }
}

#[test]
fn comment_followed_by_newline() {
    let result = lex51("x -- trailing\ny");
    assert_eq!(result.comments.len(), 1);
    assert!(result.comments[0].followed_by_newline);
}
