use luck_diagnostics::Diagnostic;
use luck_lexer::lex;
use luck_token::{Comment, CommentContent, CommentKind, CommentPosition, Span, TokenKind};

#[test]
fn line_comment_after_a_token_trails_it() {
    let result = lex(b"x -- comment");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::Line);
    assert_eq!(result.comments[0].position, CommentPosition::Trailing);
}

#[test]
fn block_comment_without_line_break_is_single_line() {
    let result = lex(b"x --[[ block ]]");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::SingleLineBlock);
}

#[test]
fn block_comment_with_line_break_is_multi_line() {
    let result = lex(b"\
x --[[ block
 comment ]]");
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::MultiLineBlock);
}

#[test]
fn block_comment_closes_at_its_own_level() {
    let result = lex(b"x --[=[ block ]] ]=]");
    assert_eq!(result.errors, Vec::<Diagnostic>::new());
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.comments[0].kind, CommentKind::SingleLineBlock);
    assert_eq!(result.comments[0].span, Span::new(2, 20));
}

/// A block comment with no closer is an error and still a comment, through
/// the end of the source, and the `Eof` token after it says it was cut off.
#[test]
fn unterminated_block_comment_is_a_comment_to_the_end_of_the_source() {
    for (source, start, kind, position) in [
        ("--[[ oops", 0, CommentKind::SingleLineBlock, CommentPosition::Leading),
        ("x = 1 --[==[ oops ]]", 6, CommentKind::SingleLineBlock, CommentPosition::Trailing),
        (
            "\
x = 1
--[[ oops
y = 2
",
            6,
            CommentKind::MultiLineBlock,
            CommentPosition::Leading,
        ),
    ] {
        let result = lex(source.as_bytes());
        let span = Span::from_indices(start, source.len());
        let eof = result.tokens.last().expect("an EOF token");
        let attached_to = match position {
            CommentPosition::Leading => eof.span.start,
            CommentPosition::Trailing => 4,
        };
        assert_eq!(
            result.errors,
            [Diagnostic::e008(span, "unterminated block comment")],
            "{source:?}"
        );
        assert_eq!(
            result.comments,
            [Comment {
                span,
                attached_to,
                kind,
                position,
                is_followed_by_newline: true,
                content: CommentContent::Plain,
            }],
            "{source:?}"
        );
        assert!(eof.is_unterminated, "{source:?}");
    }
}

#[test]
fn terminated_block_comment_leaves_the_end_whole() {
    let result = lex(b"x = [[a]] --[[ b ]]");
    assert!(result.tokens.iter().all(|token| !token.is_unterminated), "{:?}", result.tokens);
}

#[test]
fn shebang_leads_the_first_token() {
    let result = lex(b"\
#!/usr/bin/env lua
local x");
    assert_eq!(
        result.comments,
        vec![Comment {
            span: Span::new(0, 18),
            attached_to: 19,
            kind: CommentKind::Shebang,
            position: CommentPosition::Leading,
            is_followed_by_newline: true,
            content: CommentContent::Plain,
        }]
    );
}

#[test]
fn hash_after_the_first_byte_is_the_length_operator() {
    let result = lex(b"x = #t");
    assert_eq!(result.comments, Vec::<Comment>::new());
    assert!(result.tokens.iter().any(|token| token.kind == TokenKind::Hash));
}

#[test]
fn trailing_comment_attaches_to_the_previous_token() {
    let result = lex(b"local x -- trailing");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Trailing);
    assert_eq!(comment.attached_to, result.tokens[1].span.start);
}

#[test]
fn leading_comment_attaches_to_the_next_token() {
    let result = lex(b"\
-- leading
local x");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
    let local_token = &result.tokens[0];
    assert_eq!(comment.attached_to, local_token.span.start);
}

#[test]
fn comment_after_newline_is_leading() {
    let result = lex(b"\
x
-- comment
y");
    assert_eq!(result.comments.len(), 1);
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
}

#[test]
fn consecutive_leading_comments_all_lead_the_next_token() {
    let result = lex(b"\
-- first
-- second
local x");
    assert_eq!(result.comments.len(), 2);
    for comment in &result.comments {
        assert_eq!(comment.position, CommentPosition::Leading);
        assert_eq!(comment.attached_to, result.tokens[0].span.start);
    }
}

#[test]
fn comment_before_the_first_token_on_its_line_leads_it() {
    // The file start counts as a newline.
    let result = lex(b"--[[ a ]] x");
    let comment = &result.comments[0];
    assert_eq!(comment.position, CommentPosition::Leading);
    assert_eq!(comment.attached_to, 10);
    assert!(!comment.is_followed_by_newline);
}

#[test]
fn comment_at_end_of_file_leads_eof() {
    let result = lex(b"\
x
-- end");
    let eof = result.tokens.last().expect("an EOF token");
    assert_eq!(eof.kind, TokenKind::Eof);
    assert_eq!(result.comments[0].position, CommentPosition::Leading);
    assert_eq!(result.comments[0].attached_to, eof.span.start);
    assert!(result.comments[0].is_followed_by_newline);
}

#[test]
fn line_comment_is_followed_by_newline() {
    let result = lex(b"\
x -- trailing
y");
    assert_eq!(result.comments.len(), 1);
    assert!(result.comments[0].is_followed_by_newline);
}

#[test]
fn block_comment_is_followed_by_newline_only_across_spaces_and_tabs() {
    for (source, is_followed_by_newline) in
        [("x --[[ a ]] \t\ny", true), ("x --[[ a ]] \t", true), ("x --[[ a ]] y", false)]
    {
        let result = lex(source.as_bytes());
        assert_eq!(result.comments[0].is_followed_by_newline, is_followed_by_newline, "{source:?}");
    }
}

#[test]
fn comment_after_a_multi_line_block_comment_leads_the_next_token() {
    let result = lex(b"\
x --[[
]] -- c
y");
    assert_eq!(result.comments.len(), 2);
    assert_eq!(result.comments[0].position, CommentPosition::Trailing);
    assert_eq!(result.comments[0].attached_to, 0);
    assert_eq!(result.comments[1].position, CommentPosition::Leading);
    assert_eq!(result.comments[1].attached_to, 15);
}

#[test]
fn single_line_block_comment_keeps_the_next_comment_trailing() {
    let result = lex(b"\
x --[[ a ]] -- c
y");
    assert_eq!(result.comments[1].position, CommentPosition::Trailing);
}

#[test]
fn shebang_runs_past_a_carriage_return_to_the_line_feed() {
    let result = lex(b"#!x\rprint(1)\n");
    assert_eq!(
        result.tokens.len(),
        1,
        "Luau skips the first line up to LF, so nothing after the CR is code: {:?}",
        result.tokens
    );
}

#[test]
fn a_hash_first_line_without_a_bang_is_not_a_shebang() {
    let result = lex(b"\
#x
return 1");
    assert_eq!(
        result.comments,
        Vec::<Comment>::new(),
        "Luau skips only a `#!` first line, so `#x` should stay code"
    );
    assert_eq!(result.tokens[0].kind, TokenKind::Hash);
}

#[test]
fn each_comment_carries_what_it_tells_a_tool() {
    let source = "--!strict
-- luck: format off
--[[ luck: allow(x) ]]
-- luck: alow(x)
-- prose
local x";
    let contents: Vec<CommentContent> =
        lex(source.as_bytes()).comments.iter().map(|comment| comment.content).collect();
    assert_eq!(
        contents,
        [
            CommentContent::HotComment,
            CommentContent::FormatOff,
            CommentContent::LintDirective,
            CommentContent::InvalidDirective,
            CommentContent::Plain,
        ]
    );
}

#[test]
fn a_comment_or_a_shebang_holds_bytes_that_are_not_utf8_as_written() {
    let source = b"#!/bin/luau \xE9\n-- caf\xE9\nx --[[ \xFF\n\xFE ]] y";
    let result = lex(source);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let comments: Vec<(&[u8], CommentKind)> =
        result.comments.iter().map(|comment| (comment.span.text(source), comment.kind)).collect();
    assert_eq!(
        comments,
        [
            (&b"#!/bin/luau \xE9"[..], CommentKind::Shebang),
            (b"-- caf\xE9", CommentKind::Line),
            (b"--[[ \xFF\n\xFE ]]", CommentKind::MultiLineBlock),
        ]
    );
}
